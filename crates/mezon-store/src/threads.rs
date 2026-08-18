use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::AppApi;
use mezon_client::ConnectionStatus;
use mezon_client::MezonTransport;
use mezon_client::RealtimeEvent;
use mezon_client::is_channel_limit_api_error;
use mezon_client::transport::{ApiThreadDesc, THREAD_LIST_LIMIT};
use mezon_proto::{api, realtime};

use crate::channel::{Channel, ChannelEvent, ChannelList, ChannelType};
use crate::channel_members::ChannelMembersStore;
use crate::channel_permissions::{ChannelPermissionsStore, PERMISSION_MANAGE_THREAD};
use crate::clan::ClanList;
use crate::clan_members::ClanMembersStore;
use crate::ids::{ChannelId, ClanId, UserId};
use crate::messages::{
    MessagesStore, OutgoingAttachment, OutgoingContent, OutgoingEmoji, OutgoingHashtag,
    OutgoingMention, mentioned_thread_candidates, plan_thread_membership, upload_attachments_now,
};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

pub const THREAD_STATUS_ARCHIVED: i32 = 0;
pub const THREAD_STATUS_JOINED: i32 = 1;
pub const THREAD_STATUS_ACTIVE_PUBLIC: i32 = 2;
pub const THREAD_STATUS_ACTIVE_PRIVATE: i32 = 3;

pub const CHANNEL_TYPE_THREAD: u32 = 7;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(500);
const LOAD_MORE_THRESHOLD: usize = 6;
const MESSAGE_CODE_CHAT: i32 = 0;
const MESSAGE_CODE_CHAT_UPDATE: i32 = 1;
const STREAM_MODE_THREAD: i32 = 6;

#[derive(Debug, Clone)]
pub struct ThreadSummary {
    pub channel_id: String,
    pub channel_label: String,
    pub clan_id: String,
    pub parent_id: String,
    pub channel_private: i32,
    pub active: i32,
    pub creator_id: String,
    pub last_message_content: String,
    pub last_message_sender_id: String,
    pub last_message_sender_name: String,
    pub last_message_sender_avatar: String,
    pub last_sent_timestamp: i64,
    pub member_count: i32,
}

#[derive(Debug, Clone)]
pub enum ThreadsEvent {
    ThreadCreated { channel_id: String, clan_id: String },
    CreateFailed { reason: ThreadCreateFailReason },
    LeaveFailed,
    OpenPopoverRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadCreateFailReason {
    ChannelLimitExceeded,
    Other,
}

pub struct ThreadsStore {
    list_channel_id: Option<String>,
    clan_id: Option<String>,
    category_id: Option<String>,
    threads: Vec<ThreadSummary>,
    search_query: String,
    search_results: Option<Vec<ThreadSummary>>,
    search_generation: u64,
    loaded_channel: Option<String>,
    current_page: i32,
    has_more: bool,
    loading: bool,
    loading_more: bool,
    fetch_error: bool,
    searching: bool,
    creating: bool,
    submitting: bool,
    _create_task: Option<Task<()>>,
    create_private: i32,
    name_error: Option<String>,
    api: Arc<AppApi>,
    _channel_sub: Subscription,
    _conn_watch: Task<()>,
}

struct GlobalThreadsStore(Entity<ThreadsStore>);
impl Global for GlobalThreadsStore {}

impl EventEmitter<ThreadsEvent> for ThreadsStore {}

impl ThreadsStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalThreadsStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalThreadsStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalThreadsStore>()
            .map(|global| global.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let channel_sub = cx.subscribe(&ChannelList::global(cx), |this, _list, event, cx| {
            if let ChannelEvent::ActiveChannelChanged(channel_id) = event {
                this.on_active_channel_changed(*channel_id, cx);
            }
        });
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let mut store = Self {
            list_channel_id: None,
            clan_id: None,
            category_id: None,
            threads: Vec::new(),
            search_query: String::new(),
            search_results: None,
            search_generation: 0,
            loaded_channel: None,
            current_page: 0,
            has_more: false,
            loading: false,
            loading_more: false,
            fetch_error: false,
            searching: false,
            creating: false,
            submitting: false,
            _create_task: None,
            create_private: 0,
            name_error: None,
            api,
            _channel_sub: channel_sub,
            _conn_watch: conn_watch,
        };
        if let Some(channel) = ChannelList::global(cx).read(cx).active_channel() {
            store.apply_channel(channel);
        }
        store
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::ChannelCreated,
                RealtimeKind::ChannelUpdated,
                RealtimeKind::ChannelMessage,
                RealtimeKind::ChannelDeleted,
                RealtimeKind::ChannelArchive,
                RealtimeKind::UserChannelAdded,
                RealtimeKind::UserChannelRemoved,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.on_realtime_event(event, cx);
                });
            }
            dispatch.on_lagged(&entity, |this, cx| this.refresh(cx));
        });
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
                    if this
                        .update(cx, |this, cx| {
                            if this.loaded_channel.is_some() {
                                this.refresh(cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    fn on_realtime_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        if let RealtimeEvent::UserChannelAdded(ev) = event {
            self.apply_user_channel_added(ev, cx);
            return;
        }
        if let RealtimeEvent::UserChannelRemoved(ev) = event {
            self.apply_user_channel_removed(ev, cx);
            return;
        }
        let Some(list_id) = self.list_channel_id.clone() else {
            match event {
                RealtimeEvent::ChannelArchive(ev) => self.apply_channel_archive(ev, cx),
                RealtimeEvent::ChannelDeleted(ev) => self.apply_channel_deleted(ev, cx),
                _ => {}
            }
            return;
        };
        match event {
            RealtimeEvent::ChannelCreated(ev)
                if ev.parent_id != 0 && ev.parent_id.to_string() == list_id =>
            {
                self.apply_thread_created(ev, cx);
            }
            RealtimeEvent::ChannelUpdated(ev) if ev.channel_type == CHANNEL_TYPE_THREAD as i32 => {
                self.apply_thread_updated(ev, cx);
            }
            RealtimeEvent::ChannelMessage(msg) => self.apply_thread_message(msg, cx),
            RealtimeEvent::ChannelArchive(ev) => {
                self.apply_channel_archive(ev, cx);
            }
            RealtimeEvent::ChannelDeleted(ev) => self.apply_channel_deleted(ev, cx),
            _ => {}
        }
    }

    fn apply_user_channel_added(
        &mut self,
        ev: &realtime::UserChannelAdded,
        cx: &mut Context<Self>,
    ) {
        let Some(desc) = ev.channel_desc.as_ref() else {
            return;
        };
        if !is_thread_membership_event(desc.r#type, desc.parent_id) {
            return;
        }
        let Some(me) = crate::badge::BadgeService::try_global(cx)
            .and_then(|badges| badges.read(cx).current_user_id(cx))
        else {
            return;
        };
        if !ev.users.iter().any(|user| user.user_id == me.get()) {
            return;
        }
        let channel_id = desc.channel_id.to_string();
        self.set_thread_active(&channel_id, THREAD_STATUS_JOINED, cx);
        let parent_id = desc.parent_id.to_string();
        if !joins_private_thread_list(
            desc.channel_private,
            &parent_id,
            self.list_channel_id.as_deref(),
        ) {
            return;
        }
        let summary = ThreadSummary {
            channel_id,
            channel_label: desc.channel_label.clone(),
            clan_id: desc.clan_id.to_string(),
            parent_id,
            channel_private: desc.channel_private,
            active: THREAD_STATUS_JOINED,
            creator_id: desc.creator_id.to_string(),
            last_message_content: String::new(),
            last_message_sender_id: String::new(),
            last_message_sender_name: String::new(),
            last_message_sender_avatar: String::new(),
            last_sent_timestamp: match i64::from(ev.create_time_seconds) {
                0 => crate::message_time::unix_now_seconds(),
                seconds => seconds,
            },
            member_count: desc.member_count.max(0),
        };
        let before = self.threads.len();
        merge_threads(&mut self.threads, vec![summary.clone()]);
        let mut changed = self.threads.len() != before;
        if let Some(results) = self.search_results.as_mut() {
            let results_before = results.len();
            merge_threads(results, vec![summary]);
            changed |= results.len() != results_before;
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_user_channel_removed(
        &mut self,
        ev: &realtime::UserChannelRemoved,
        cx: &mut Context<Self>,
    ) {
        if ev.channel_type != CHANNEL_TYPE_THREAD as i32 {
            return;
        }
        let Some(me) = crate::badge::BadgeService::try_global(cx)
            .and_then(|badges| badges.read(cx).current_user_id(cx))
        else {
            return;
        };
        if !ev.user_ids.contains(&me.get()) {
            return;
        }
        let channel_id = ev.channel_id.to_string();
        if !self.is_private_thread(&channel_id) {
            return;
        }
        self.apply_thread_deleted(&channel_id, cx);
    }

    fn is_private_thread(&self, channel_id: &str) -> bool {
        contains_private_thread(
            self.threads
                .iter()
                .chain(self.search_results.iter().flatten()),
            channel_id,
        )
    }

    pub fn leave_thread(&mut self, clan_id: ClanId, channel_id: ChannelId, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let is_private = ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .map(|channel| channel.private)
            .unwrap_or_else(|| self.is_private_thread(&channel_id.to_string()));
        cx.spawn(async move |this, cx| {
            if let Err(e) = api.leave_thread(clan_id.get(), channel_id.get()).await {
                tracing::error!("leave_thread failed for {channel_id}: {e}");
                let _ = this.update(cx, |_this, cx| {
                    cx.emit(ThreadsEvent::LeaveFailed);
                });
                return;
            }
            let _ = this.update(cx, |this, cx| {
                ChannelList::global(cx).update(cx, |list, cx| {
                    list.apply_self_removed_from_channel(channel_id, cx);
                });
                if is_private {
                    this.apply_thread_deleted(&channel_id.to_string(), cx);
                }
            });
        })
        .detach();
    }

    pub fn mark_thread_active(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        self.set_thread_active(channel_id, THREAD_STATUS_JOINED, cx);
    }

    pub fn mark_thread_archived(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        self.set_thread_active(channel_id, THREAD_STATUS_ARCHIVED, cx);
    }

    pub fn remove_thread_locally(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        self.apply_thread_deleted(channel_id, cx);
    }

    pub fn remove_threads_of_parent(&mut self, parent_id: &str, cx: &mut Context<Self>) {
        self.apply_parent_channel_deleted(parent_id, cx);
    }

    #[cfg(test)]
    pub(crate) fn seed_threads_for_test(
        &mut self,
        list_channel_id: &str,
        threads: Vec<ThreadSummary>,
        cx: &mut Context<Self>,
    ) {
        self.list_channel_id = Some(list_channel_id.to_string());
        self.threads = threads;
        cx.notify();
    }

    pub fn thread_active(&self, channel_id: &str) -> Option<i32> {
        self.threads
            .iter()
            .find(|t| t.channel_id == channel_id)
            .map(|t| t.active)
            .or_else(|| {
                self.search_results.as_ref().and_then(|results| {
                    results
                        .iter()
                        .find(|t| t.channel_id == channel_id)
                        .map(|t| t.active)
                })
            })
    }

    fn set_thread_active(&mut self, channel_id: &str, active: i32, cx: &mut Context<Self>) {
        let mut changed = false;
        if let Some(thread) = self.threads.iter_mut().find(|t| t.channel_id == channel_id)
            && thread.active != active
        {
            thread.active = active;
            changed = true;
        }
        if let Some(results) = self.search_results.as_mut()
            && let Some(thread) = results.iter_mut().find(|t| t.channel_id == channel_id)
            && thread.active != active
        {
            thread.active = active;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_channel_archive(
        &mut self,
        ev: &realtime::ChannelArchiveEvent,
        cx: &mut Context<Self>,
    ) {
        let channel_id = ev.channel_id.to_string();
        if ev.active == THREAD_STATUS_ARCHIVED {
            self.mark_thread_archived(&channel_id, cx);
        } else {
            self.mark_thread_active(&channel_id, cx);
        }
    }

    fn apply_channel_deleted(
        &mut self,
        ev: &realtime::ChannelDeletedEvent,
        cx: &mut Context<Self>,
    ) {
        let channel_id = ev.channel_id.to_string();
        if ev.parent_id != 0 {
            self.apply_thread_deleted(&channel_id, cx);
            return;
        }
        self.apply_parent_channel_deleted(&channel_id, cx);
    }

    fn apply_parent_channel_deleted(&mut self, parent_id: &str, cx: &mut Context<Self>) {
        let before = self.threads.len();
        self.threads.retain(|t| t.parent_id != parent_id);
        let mut changed = self.threads.len() != before;
        if let Some(results) = self.search_results.as_mut() {
            let results_before = results.len();
            results.retain(|t| t.parent_id != parent_id);
            changed |= results.len() != results_before;
        }
        if self.list_channel_id.as_deref() == Some(parent_id) {
            self.list_channel_id = None;
            self.loaded_channel = None;
            self.search_results = None;
            self.search_query.clear();
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_thread_message(&mut self, msg: &api::ChannelMessage, cx: &mut Context<Self>) {
        let channel_id = msg.channel_id.to_string();
        if !self.threads.iter().any(|t| t.channel_id == channel_id)
            && !self
                .search_results
                .as_ref()
                .is_some_and(|results| results.iter().any(|t| t.channel_id == channel_id))
        {
            return;
        }
        let mut changed = false;
        if patch_thread_in_list(&mut self.threads, &channel_id, msg) {
            changed = true;
        }
        if let Some(results) = self.search_results.as_mut()
            && patch_thread_in_list(results, &channel_id, msg)
        {
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_thread_updated(&mut self, ev: &realtime::ChannelUpdatedEvent, cx: &mut Context<Self>) {
        let channel_id = ev.channel_id.to_string();
        let mut changed = false;
        for thread in self
            .threads
            .iter_mut()
            .filter(|t| t.channel_id == channel_id)
        {
            patch_thread_from_updated(thread, ev);
            changed = true;
        }
        if let Some(results) = self.search_results.as_mut() {
            for thread in results.iter_mut().filter(|t| t.channel_id == channel_id) {
                patch_thread_from_updated(thread, ev);
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_thread_deleted(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        let before = self.threads.len();
        self.threads.retain(|t| t.channel_id != channel_id);
        let mut changed = self.threads.len() != before;
        if let Some(results) = self.search_results.as_mut() {
            let results_before = results.len();
            results.retain(|t| t.channel_id != channel_id);
            changed |= results.len() != results_before;
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_thread_created(&mut self, ev: &realtime::ChannelCreatedEvent, cx: &mut Context<Self>) {
        if ev.channel_type != CHANNEL_TYPE_THREAD as i32 {
            return;
        }
        let Some(summary) = filter_threads(vec![thread_from_created_event(ev)])
            .into_iter()
            .next()
        else {
            return;
        };
        let before = self.threads.len();
        merge_threads(&mut self.threads, vec![summary.clone()]);
        let mut changed = self.threads.len() != before;
        if let Some(results) = self.search_results.as_mut() {
            let results_before = results.len();
            merge_threads(results, vec![summary]);
            changed |= results.len() != results_before;
        }
        if changed {
            cx.notify();
        }
    }

    pub fn threads(&self) -> &[ThreadSummary] {
        &self.threads
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn search_results(&self) -> Option<&[ThreadSummary]> {
        self.search_results.as_deref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_loading_more(&self) -> bool {
        self.loading_more
    }

    pub fn fetch_error(&self) -> bool {
        self.fetch_error
    }

    pub fn retry_fetch(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
    }

    pub fn has_more(&self) -> bool {
        self.has_more
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    pub fn is_creating(&self) -> bool {
        self.creating
    }

    pub fn is_submitting(&self) -> bool {
        self.submitting
    }

    pub fn create_private(&self) -> bool {
        self.create_private != 0
    }

    pub fn set_create_private(&mut self, private: bool, cx: &mut Context<Self>) {
        self.create_private = i32::from(private);
        cx.notify();
    }

    pub fn name_error(&self) -> Option<&str> {
        self.name_error.as_deref()
    }

    pub fn show_threads_popover(&self, cx: &App) -> bool {
        self.list_channel_id
            .as_ref()
            .and_then(|id| id.parse::<ChannelId>().ok())
            .and_then(|id| {
                ChannelList::global(cx)
                    .read(cx)
                    .find_channel_in_active_clan(id)
            })
            .is_some_and(|ch| {
                !matches!(ch.channel_type, ChannelType::Thread)
                    && matches!(
                        ch.channel_type,
                        ChannelType::Text | ChannelType::Forum | ChannelType::Announcement
                    )
            })
    }

    fn apply_channel(&mut self, channel: &Channel) {
        self.list_channel_id = Some(list_channel_id_for(channel));
        self.clan_id = Some(channel.clan_id.to_string());
        self.category_id = channel.category_id.clone();
    }

    fn sync_list_scope(&mut self, cx: &App) {
        let Some(active_id) = ChannelList::global(cx).read(cx).active_channel_id else {
            return;
        };
        if let Some(channel) = ChannelList::global(cx)
            .read(cx)
            .find_channel_in_active_clan(active_id)
        {
            self.apply_channel(channel);
            return;
        }
        self.list_channel_id = Some(active_id.to_string());
        self.clan_id = Some("0".to_string());
    }

    fn resolve_fetch_clan_id(&self) -> Option<String> {
        self.clan_id.as_ref().filter(|id| !id.is_empty()).cloned()
    }

    fn invalidate_create_request(&mut self) {
        self._create_task = None;
        self.submitting = false;
    }

    fn finish_create_request(&mut self) {
        self.submitting = false;
        self._create_task = None;
    }

    fn on_active_channel_changed(&mut self, channel_id: Option<ChannelId>, cx: &mut Context<Self>) {
        match channel_id {
            None => {
                self.list_channel_id = None;
                self.clan_id = None;
                self.category_id = None;
            }
            Some(id) => {
                if let Some(channel) = ChannelList::global(cx)
                    .read(cx)
                    .find_channel_in_active_clan(id)
                {
                    self.apply_channel(channel);
                } else {
                    self.list_channel_id = Some(id.to_string());
                    self.clan_id = Some("0".to_string());
                    self.category_id = None;
                }
            }
        }
        self.threads.clear();
        self.search_query.clear();
        self.search_results = None;
        self.search_generation = self.search_generation.wrapping_add(1);
        self.searching = false;
        self.loaded_channel = None;
        self.current_page = 0;
        self.has_more = false;
        self.loading_more = false;
        self.fetch_error = false;
        self.invalidate_create_request();
        self.name_error = None;
        cx.notify();
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.list_channel_id.clone() else {
            return;
        };
        if self.loading || self.loaded_channel.as_deref() == Some(channel_id.as_str()) {
            return;
        }
        self.fetch(cx);
    }

    pub fn request_open_popover(&mut self, cx: &mut Context<Self>) {
        cx.emit(ThreadsEvent::OpenPopoverRequested);
    }

    pub fn open_popover(&mut self, cx: &mut Context<Self>) {
        self.sync_list_scope(cx);
        self.ensure_create_permissions(cx);
        self.refresh(cx);
    }

    pub fn list_clan_id(&self) -> Option<&str> {
        self.clan_id.as_deref()
    }

    pub fn list_channel_id(&self) -> Option<&str> {
        self.list_channel_id.as_deref()
    }

    pub fn can_create_thread(&self, cx: &App) -> bool {
        let is_dm = MessagesStore::try_global(cx).is_some_and(|store| store.read(cx).is_dm());
        let Some((channel_id, clan_id)) = thread_creation_scope(
            is_dm,
            self.list_channel_id.as_deref(),
            self.clan_id.as_deref(),
        ) else {
            return false;
        };
        let Some(clan_id) = clan_id.or_else(|| ClanList::global(cx).read(cx).active_clan_id) else {
            return false;
        };
        ChannelPermissionsStore::global(cx).read(cx).has_permission(
            PERMISSION_MANAGE_THREAD,
            clan_id,
            channel_id,
        )
    }

    pub fn ensure_create_permissions(&mut self, cx: &mut Context<Self>) {
        let is_dm = MessagesStore::try_global(cx).is_some_and(|store| store.read(cx).is_dm());
        let Some((channel_id, clan_id)) = thread_creation_scope(
            is_dm,
            self.list_channel_id.as_deref(),
            self.clan_id.as_deref(),
        ) else {
            return;
        };
        let Some(clan_id) = clan_id.or_else(|| ClanList::global(cx).read(cx).active_clan_id) else {
            return;
        };
        ChannelPermissionsStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, channel_id, cx);
        });
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loaded_channel = None;
        self.current_page = 0;
        self.has_more = false;
        self.fetch(cx);
    }

    pub fn maybe_load_more(&mut self, visible_end: usize, cx: &mut Context<Self>) {
        if !self.search_query.trim().is_empty() {
            return;
        }
        let count = self.threads.len();
        if count == 0 || count.saturating_sub(visible_end) > LOAD_MORE_THRESHOLD {
            return;
        }
        self.fetch_more(cx);
    }

    pub fn fetch_more(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.loading_more || !self.has_more {
            return;
        }
        self.fetch_page(self.current_page + 1, true, cx);
    }

    pub fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        if query.trim().is_empty() {
            if self.search_query.is_empty() && self.search_results.is_none() && !self.searching {
                return;
            }
            self.search_query = query;
            self.search_results = None;
            self.searching = false;
            self.search_generation = self.search_generation.wrapping_add(1);
            cx.notify();
            return;
        }
        self.search_query = query.clone();
        self.searching = true;
        cx.notify();
        self.schedule_search(cx, query);
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>, query: String) {
        self.sync_list_scope(cx);
        let Some(channel_id) = self.list_channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.resolve_fetch_clan_id() else {
            return;
        };
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        let trimmed = query.trim().to_string();
        let api = self.api.clone();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            if this
                .update(cx, |this, _| {
                    this.search_generation != generation
                        || this.search_query.trim() != trimmed
                        || this.list_channel_id.as_deref() != Some(channel_id.as_str())
                })
                .unwrap_or(true)
            {
                return;
            }

            let result = api.search_thread(&clan_id, &channel_id, &trimmed).await;

            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                if this.search_query.trim() != trimmed {
                    return;
                }
                if this.list_channel_id.as_deref() != Some(channel_id.as_str()) {
                    this.searching = false;
                    cx.notify();
                    return;
                }
                this.searching = false;
                match result {
                    Ok(list) => {
                        let results: Vec<ThreadSummary> =
                            list.into_iter().map(thread_from_api).collect();
                        this.ensure_clan_members_for_threads(&results, cx);
                        this.search_results = Some(results);
                    }
                    Err(e) => tracing::error!("search_thread failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_clan_members_for_threads(&self, threads: &[ThreadSummary], cx: &mut Context<Self>) {
        let mut seen = std::collections::HashSet::new();
        for thread in threads {
            let Ok(clan_id) = thread.clan_id.parse::<ClanId>() else {
                continue;
            };
            if seen.insert(clan_id) {
                ClanMembersStore::global(cx).update(cx, |store, cx| {
                    store.ensure_loaded(clan_id, cx);
                });
            }
        }
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.fetch_page(1, false, cx);
    }

    fn fetch_page(&mut self, page: i32, append: bool, cx: &mut Context<Self>) {
        self.sync_list_scope(cx);
        let Some(channel_id) = self.list_channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.resolve_fetch_clan_id() else {
            tracing::warn!(
                target: "mezon.threads",
                channel_id = %channel_id,
                "list_thread_descs skipped: clan_id unavailable"
            );
            return;
        };
        if append {
            if self.loading_more || !self.has_more {
                return;
            }
            self.loading_more = true;
        } else if self.loading {
            return;
        } else {
            self.loading = true;
            self.fetch_error = false;
        }
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_thread_descs(&channel_id, &clan_id, page).await;
            let _ = this.update(cx, |this, cx| {
                if append {
                    this.loading_more = false;
                } else {
                    this.loading = false;
                }
                if this.list_channel_id.as_deref() != Some(channel_id.as_str()) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(list) => {
                        let page_full = page_has_more(list.len());
                        let batch = filter_threads(list.into_iter().map(thread_from_api).collect());
                        this.ensure_clan_members_for_threads(&batch, cx);
                        if append {
                            merge_threads(&mut this.threads, batch);
                        } else {
                            this.threads = batch;
                            this.loaded_channel = Some(channel_id);
                            this.fetch_error = false;
                        }
                        this.current_page = page;
                        this.has_more = page_full;
                    }
                    Err(e) => {
                        tracing::error!("list_thread_descs page {page} failed: {e}");
                        if !append {
                            this.fetch_error = true;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn start_create(&mut self, cx: &mut Context<Self>) {
        if !self.can_create_thread(cx) {
            return;
        }
        self.creating = true;
        self.submitting = false;
        self.create_private = 0;
        self.name_error = None;
        cx.notify();
    }

    pub fn cancel_create(&mut self, cx: &mut Context<Self>) {
        if !self.creating
            && self._create_task.is_none()
            && !self.submitting
            && self.create_private == 0
            && self.name_error.is_none()
        {
            return;
        }
        self.creating = false;
        self.invalidate_create_request();
        self.create_private = 0;
        self.name_error = None;
        cx.notify();
    }

    pub fn submit_create(
        &mut self,
        name: String,
        message: String,
        content_tokens: OutgoingContent,
        attachments: Vec<OutgoingAttachment>,
        cx: &mut Context<Self>,
    ) {
        if self._create_task.is_some() || self.submitting || !self.can_create_thread(cx) {
            return;
        }
        let name = name.trim().to_string();
        if name.is_empty() {
            self.name_error = Some("thread_name_too_short".into());
            cx.notify();
            return;
        }
        if message.trim().is_empty() && attachments.is_empty() {
            self.name_error = Some("initial_message_required".into());
            cx.notify();
            return;
        }
        let message = message.trim_end().to_string();

        let Some(parent_id) = self.list_channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return;
        };
        let Ok(clan_id_parsed) = clan_id.parse::<ClanId>() else {
            return;
        };
        if clan_id_parsed.is_zero() {
            return;
        }
        let Ok(parent_channel_id) = parent_id.parse::<ChannelId>() else {
            return;
        };
        let category_id = self.category_id.clone();
        let channel_private = self.create_private;
        let clan_id_i64 = clan_id_parsed.get();
        let parent_channel_type = ChannelList::global(cx)
            .read(cx)
            .channel(clan_id_parsed, parent_channel_id)
            .map(|channel| channel.channel_type.as_raw() as i32);
        let cached_parent_members = ChannelMembersStore::try_global(cx).and_then(|members| {
            let members = members.read(cx);
            members
                .has_channel(parent_channel_id)
                .then(|| members.member_ids(parent_channel_id))
        });
        let transport_mentions = content_tokens
            .mentions
            .into_iter()
            .map(OutgoingMention::into_transport)
            .collect::<Vec<_>>();
        let mentioned = mentioned_thread_candidates(&transport_mentions, clan_id_parsed, cx);
        let transport_hashtags = content_tokens
            .hashtags
            .into_iter()
            .map(OutgoingHashtag::into_transport)
            .collect::<Vec<_>>();
        let transport_emojis = content_tokens
            .emojis
            .into_iter()
            .map(OutgoingEmoji::into_transport)
            .collect::<Vec<_>>();

        self.name_error = None;
        self.creating = true;
        self.submitting = true;
        cx.notify();

        let api = self.api.clone();
        let task = cx.spawn(async move |this, cx| {
            match api.check_duplicate_thread_name(&name, &parent_id).await {
                Ok(true) => {
                    if let Err(e) = this.update(cx, |this, cx| {
                        this.finish_create_request();
                        this.name_error = Some("thread_name_exists".into());
                        cx.notify();
                    }) {
                        tracing::error!("threads create state update failed: {e}");
                    }
                    return;
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::error!("check_duplicate_thread_name failed: {e}");
                    if let Err(e) = this.update(cx, |this, cx| {
                        this.finish_create_request();
                        cx.emit(ThreadsEvent::CreateFailed {
                            reason: thread_create_fail_reason(&e),
                        });
                        cx.notify();
                    }) {
                        tracing::error!("threads create state update failed: {e}");
                    }
                    return;
                }
            }

            let create_result = api
                .create_channel(
                    clan_id_i64,
                    &name,
                    CHANNEL_TYPE_THREAD,
                    category_id.as_deref().and_then(|s| s.parse().ok()),
                    Some(parent_channel_id.get()),
                    channel_private,
                )
                .await;

            let thread = match create_result {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("create_channel (thread) failed: {e}");
                    if let Err(e) = this.update(cx, |this, cx| {
                        this.finish_create_request();
                        cx.emit(ThreadsEvent::CreateFailed {
                            reason: thread_create_fail_reason(&e),
                        });
                        cx.notify();
                    }) {
                        tracing::error!("threads create state update failed: {e}");
                    }
                    return;
                }
            };

            let thread_id = thread.channel_id;
            let thread_id_str = thread_id.to_string();

            if let Err(e) = api
                .join_chat(clan_id_i64, thread_id, CHANNEL_TYPE_THREAD as i32, false)
                .await
            {
                tracing::warn!("join_chat after thread create failed: {e}");
            }

            let parent_members = match cached_parent_members {
                Some(ids) => ids,
                None => match parent_channel_type {
                    Some(channel_type) => {
                        match api
                            .list_channel_users(clan_id_i64, parent_channel_id.get(), channel_type)
                            .await
                        {
                            Ok(users) => {
                                let ids: Vec<UserId> =
                                    users.iter().map(|user| UserId(user.user_id)).collect();
                                let _ = this.update(cx, |_this, cx| {
                                    if let Some(members) = ChannelMembersStore::try_global(cx) {
                                        members.update(cx, |members, cx| {
                                            members.apply_members_loaded(
                                                parent_channel_id,
                                                &users,
                                                cx,
                                            );
                                        });
                                    }
                                });
                                ids
                            }
                            Err(e) => {
                                tracing::error!(
                                    "list parent members for thread create failed: {e}"
                                );
                                Vec::new()
                            }
                        }
                    }
                    None => Vec::new(),
                },
            };
            let invite_ids = plan_thread_membership(None, &[], &parent_members, &mentioned);

            let mut invite_failed = false;
            if !invite_ids.is_empty() {
                let user_ids: Vec<String> = invite_ids
                    .iter()
                    .map(|user_id| user_id.to_string())
                    .collect();
                if let Err(e) = api.add_channel_users(thread_id, user_ids).await {
                    tracing::error!("add mentioned users to new thread failed: {e}");
                    invite_failed = true;
                }
            }
            let starter_mentions = if channel_private != 0 && invite_failed {
                Vec::new()
            } else {
                transport_mentions
            };

            let send_result = if attachments.is_empty() {
                api.send_channel_message(
                    clan_id_i64,
                    thread_id,
                    &message,
                    false,
                    STREAM_MODE_THREAD,
                    starter_mentions,
                    transport_hashtags,
                    transport_emojis,
                    None,
                )
                .await
            } else {
                match upload_attachments_now(&api, attachments).await {
                    Ok(uploaded) => {
                        api.send_presigned_message(
                            clan_id_i64,
                            thread_id,
                            &message,
                            false,
                            STREAM_MODE_THREAD,
                            uploaded,
                            None,
                            starter_mentions,
                            transport_hashtags,
                            transport_emojis,
                            None,
                            Default::default(),
                        )
                        .await
                    }
                    Err(e) => Err(e),
                }
            };
            if let Err(e) = send_result {
                tracing::error!("send starter message to thread failed: {e}");
                if let Err(e) = this.update(cx, |this, cx| {
                    this.finish_create_request();
                    cx.emit(ThreadsEvent::CreateFailed {
                        reason: ThreadCreateFailReason::Other,
                    });
                    cx.notify();
                }) {
                    tracing::error!("threads create state update failed: {e}");
                }
                return;
            }

            if let Err(e) = this.update(cx, |this, cx| {
                this.creating = false;
                this.finish_create_request();
                this.loaded_channel = None;
                ChannelList::global(cx).update(cx, |list, cx| {
                    list.refresh_clan(clan_id_parsed, cx);
                });
                cx.emit(ThreadsEvent::ThreadCreated {
                    channel_id: thread_id_str,
                    clan_id: clan_id.clone(),
                });
                cx.notify();
            }) {
                tracing::error!("threads create state update failed: {e}");
            }
        });
        self._create_task = Some(task);
    }
}

fn thread_create_fail_reason(err: &anyhow::Error) -> ThreadCreateFailReason {
    if is_channel_limit_api_error(err) {
        ThreadCreateFailReason::ChannelLimitExceeded
    } else {
        ThreadCreateFailReason::Other
    }
}

fn list_channel_id_for(channel: &Channel) -> String {
    if channel.channel_type == ChannelType::Thread {
        channel
            .parent_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| channel.id.to_string())
    } else {
        channel.id.to_string()
    }
}

fn thread_creation_scope(
    is_dm: bool,
    list_channel_id: Option<&str>,
    clan_id: Option<&str>,
) -> Option<(ChannelId, Option<ClanId>)> {
    if is_dm {
        return None;
    }
    let channel_id = list_channel_id?.parse::<ChannelId>().ok()?;
    let clan_id = clan_id
        .and_then(|raw| raw.parse::<ClanId>().ok())
        .filter(|id| !id.is_zero());
    Some((channel_id, clan_id))
}

fn page_has_more(batch_len: usize) -> bool {
    batch_len >= THREAD_LIST_LIMIT as usize
}

/// A `UserChannelAdded`/`UserChannelRemoved` payload only concerns the thread
/// list when it describes a thread hanging off a real parent channel.
fn is_thread_membership_event(channel_type: i32, parent_id: i64) -> bool {
    channel_type == CHANNEL_TYPE_THREAD as i32 && parent_id != 0
}

/// Being added is the only signal that can surface a *private* thread, so it is
/// the only case that inserts a row. Public threads already arrive through
/// `ChannelCreated`, and only the list currently showing the parent can grow.
fn joins_private_thread_list(
    channel_private: i32,
    parent_id: &str,
    list_channel_id: Option<&str>,
) -> bool {
    channel_private != 0 && list_channel_id == Some(parent_id)
}

fn contains_private_thread<'a>(
    mut threads: impl Iterator<Item = &'a ThreadSummary>,
    channel_id: &str,
) -> bool {
    threads.any(|thread| thread.channel_id == channel_id && thread.channel_private != 0)
}

fn merge_threads(into: &mut Vec<ThreadSummary>, batch: Vec<ThreadSummary>) {
    for thread in batch {
        if !into
            .iter()
            .any(|existing| existing.channel_id == thread.channel_id)
        {
            into.push(thread);
        }
    }
}

fn filter_threads(threads: Vec<ThreadSummary>) -> Vec<ThreadSummary> {
    threads
        .into_iter()
        .filter(|t| {
            if t.channel_private != 0 {
                t.active == THREAD_STATUS_JOINED
                    || t.active == THREAD_STATUS_ACTIVE_PRIVATE
                    || t.active == THREAD_STATUS_ARCHIVED
            } else {
                true
            }
        })
        .collect()
}

fn thread_from_api(t: ApiThreadDesc) -> ThreadSummary {
    ThreadSummary {
        channel_id: t.channel_id,
        channel_label: t.channel_label,
        clan_id: t.clan_id,
        parent_id: t.parent_id,
        channel_private: t.channel_private,
        active: t.active,
        creator_id: t.creator_id,
        last_message_content: t.last_message_content,
        last_message_sender_id: t.last_message_sender_id,
        last_message_sender_name: t.last_message_sender_name,
        last_message_sender_avatar: t.last_message_sender_avatar,
        last_sent_timestamp: t.last_sent_timestamp,
        member_count: t.member_count,
    }
}

fn thread_from_created_event(ev: &realtime::ChannelCreatedEvent) -> ThreadSummary {
    ThreadSummary {
        channel_id: ev.channel_id.to_string(),
        channel_label: ev.channel_label.clone(),
        clan_id: ev.clan_id.to_string(),
        parent_id: ev.parent_id.to_string(),
        channel_private: ev.channel_private,
        active: ev.status,
        creator_id: ev.creator_id.to_string(),
        last_message_content: String::new(),
        last_message_sender_id: ev.creator_id.to_string(),
        last_message_sender_name: String::new(),
        last_message_sender_avatar: String::new(),
        last_sent_timestamp: 0,
        member_count: 0,
    }
}

fn patch_thread_from_updated(thread: &mut ThreadSummary, ev: &realtime::ChannelUpdatedEvent) {
    if !ev.channel_label.is_empty() {
        thread.channel_label = ev.channel_label.clone();
    }
    thread.active = ev.status;
    thread.channel_private = i32::from(ev.channel_private);
}

fn patch_thread_from_message(thread: &mut ThreadSummary, msg: &api::ChannelMessage) {
    thread.last_sent_timestamp = i64::from(msg.create_time_seconds);
    if msg.code == MESSAGE_CODE_CHAT || msg.code == MESSAGE_CODE_CHAT_UPDATE {
        let api_msg = MezonTransport::message_from_proto(msg);
        thread.last_message_content = api_msg.content;
        thread.last_message_sender_id = api_msg.sender_id.to_string();
        thread.last_message_sender_name = api_msg.sender_name;
        thread.last_message_sender_avatar = api_msg.avatar;
    }
}

fn patch_thread_in_list(
    threads: &mut [ThreadSummary],
    channel_id: &str,
    msg: &api::ChannelMessage,
) -> bool {
    let Some(thread) = threads.iter_mut().find(|t| t.channel_id == channel_id) else {
        return false;
    };
    patch_thread_from_message(thread, msg);
    true
}

pub struct GroupedThreadIndexes {
    pub joined: Vec<usize>,
    pub active: Vec<usize>,
    pub archived: Vec<usize>,
}

pub fn group_threads(threads: &[ThreadSummary]) -> GroupedThreadIndexes {
    let mut joined = Vec::new();
    let mut active = Vec::new();
    let mut archived = Vec::new();

    for (index, t) in threads.iter().enumerate() {
        match t.active {
            THREAD_STATUS_JOINED => joined.push(index),
            THREAD_STATUS_ARCHIVED => archived.push(index),
            _ => active.push(index),
        }
    }

    let sort = |v: &mut Vec<usize>| {
        v.sort_by_key(|&i| std::cmp::Reverse(threads[i].last_sent_timestamp));
    };
    sort(&mut joined);
    sort(&mut active);
    sort(&mut archived);

    GroupedThreadIndexes {
        joined,
        active,
        archived,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_creation_scope_is_none_in_a_dm() {
        assert_eq!(thread_creation_scope(true, Some("77"), Some("5")), None);
        assert_eq!(thread_creation_scope(true, Some("77"), Some("0")), None);
    }

    #[test]
    fn thread_creation_scope_drops_a_zero_clan_so_the_caller_can_fall_back() {
        assert_eq!(
            thread_creation_scope(false, Some("77"), Some("0")),
            Some((ChannelId(77), None))
        );
        assert_eq!(
            thread_creation_scope(false, Some("77"), None),
            Some((ChannelId(77), None))
        );
    }

    #[test]
    fn thread_creation_scope_keeps_a_real_clan_channel() {
        assert_eq!(
            thread_creation_scope(false, Some("77"), Some("5")),
            Some((ChannelId(77), Some(ClanId(5))))
        );
        assert_eq!(thread_creation_scope(false, None, Some("5")), None);
        assert_eq!(thread_creation_scope(false, Some("nope"), Some("5")), None);
    }

    #[test]
    fn page_has_more_when_batch_full() {
        assert!(page_has_more(THREAD_LIST_LIMIT as usize));
    }

    #[test]
    fn page_has_more_false_when_batch_partial() {
        assert!(!page_has_more(10));
    }

    fn summary(channel_id: &str, channel_private: i32) -> ThreadSummary {
        ThreadSummary {
            channel_id: channel_id.into(),
            channel_label: "t".into(),
            clan_id: "c".into(),
            parent_id: "p".into(),
            channel_private,
            active: THREAD_STATUS_JOINED,
            creator_id: String::new(),
            last_message_content: String::new(),
            last_message_sender_id: String::new(),
            last_message_sender_name: String::new(),
            last_message_sender_avatar: String::new(),
            last_sent_timestamp: 0,
            member_count: 0,
        }
    }

    #[test]
    fn membership_events_only_match_threads_with_a_parent() {
        assert!(is_thread_membership_event(CHANNEL_TYPE_THREAD as i32, 77));
        assert!(!is_thread_membership_event(CHANNEL_TYPE_THREAD as i32, 0));
        assert!(!is_thread_membership_event(
            CHANNEL_TYPE_THREAD as i32 + 1,
            77
        ));
    }

    #[test]
    fn only_a_private_thread_under_the_open_parent_joins_the_list() {
        assert!(joins_private_thread_list(1, "77", Some("77")));
        // Public threads arrive via ChannelCreated instead.
        assert!(!joins_private_thread_list(0, "77", Some("77")));
        // A different parent's list must not grow.
        assert!(!joins_private_thread_list(1, "77", Some("88")));
        assert!(!joins_private_thread_list(1, "77", None));
    }

    #[test]
    fn private_thread_lookup_spans_the_list_and_search_results() {
        let threads = [summary("1", 0)];
        let results = [summary("2", 1)];

        assert!(contains_private_thread(
            threads.iter().chain(results.iter()),
            "2"
        ));
        // Public threads are left alone: removal must not delete their row.
        assert!(!contains_private_thread(
            threads.iter().chain(results.iter()),
            "1"
        ));
        assert!(!contains_private_thread(
            threads.iter().chain(results.iter()),
            "9"
        ));
    }

    #[test]
    fn filter_threads_keeps_archived_private_threads_in_popover() {
        let threads = vec![
            summary("1", 1),
            {
                let mut thread = summary("2", 0);
                thread.active = THREAD_STATUS_ARCHIVED;
                thread
            },
            {
                let mut thread = summary("3", 1);
                thread.active = THREAD_STATUS_ARCHIVED;
                thread
            },
        ];
        let kept = filter_threads(threads);
        assert_eq!(kept.len(), 3);
        assert!(kept.iter().any(|t| t.channel_id == "1"));
        assert!(kept.iter().any(|t| t.channel_id == "2"));
        assert!(kept.iter().any(|t| t.channel_id == "3"));
    }

    #[test]
    fn filter_threads_drops_private_threads_user_has_not_joined() {
        let threads = vec![{
            let mut thread = summary("9", 1);
            thread.active = THREAD_STATUS_ACTIVE_PUBLIC;
            thread
        }];
        assert!(filter_threads(threads).is_empty());
    }

    #[test]
    fn merge_threads_dedupes_by_channel_id() {
        let mut threads = vec![ThreadSummary {
            channel_id: "1".into(),
            channel_label: "a".into(),
            clan_id: "c".into(),
            parent_id: "p".into(),
            channel_private: 0,
            active: THREAD_STATUS_JOINED,
            creator_id: String::new(),
            last_message_content: String::new(),
            last_message_sender_id: String::new(),
            last_message_sender_name: String::new(),
            last_message_sender_avatar: String::new(),
            last_sent_timestamp: 0,
            member_count: 0,
        }];
        merge_threads(
            &mut threads,
            vec![
                ThreadSummary {
                    channel_id: "1".into(),
                    channel_label: "dup".into(),
                    clan_id: "c".into(),
                    parent_id: "p".into(),
                    channel_private: 0,
                    active: THREAD_STATUS_JOINED,
                    creator_id: String::new(),
                    last_message_content: String::new(),
                    last_message_sender_id: String::new(),
                    last_message_sender_name: String::new(),
                    last_message_sender_avatar: String::new(),
                    last_sent_timestamp: 0,
                    member_count: 0,
                },
                ThreadSummary {
                    channel_id: "2".into(),
                    channel_label: "b".into(),
                    clan_id: "c".into(),
                    parent_id: "p".into(),
                    channel_private: 0,
                    active: THREAD_STATUS_JOINED,
                    creator_id: String::new(),
                    last_message_content: String::new(),
                    last_message_sender_id: String::new(),
                    last_message_sender_name: String::new(),
                    last_message_sender_avatar: String::new(),
                    last_sent_timestamp: 0,
                    member_count: 0,
                },
            ],
        );
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].channel_label, "a");
    }

    #[test]
    fn patch_thread_from_message_updates_preview_on_chat() {
        let mut thread = ThreadSummary {
            channel_id: "1".into(),
            channel_label: "t".into(),
            clan_id: "c".into(),
            parent_id: "p".into(),
            channel_private: 0,
            active: THREAD_STATUS_JOINED,
            creator_id: String::new(),
            last_message_content: "old".into(),
            last_message_sender_id: "9".into(),
            last_message_sender_name: String::new(),
            last_message_sender_avatar: String::new(),
            last_sent_timestamp: 1,
            member_count: 0,
        };
        let msg = api::ChannelMessage {
            channel_id: 1,
            sender_id: 42,
            content: r#"{"t":"hello"}"#.into(),
            create_time_seconds: 99,
            code: MESSAGE_CODE_CHAT,
            ..Default::default()
        };
        patch_thread_from_message(&mut thread, &msg);
        assert_eq!(thread.last_sent_timestamp, 99);
        assert_eq!(thread.last_message_content, "hello");
        assert_eq!(thread.last_message_sender_id, "42");
    }

    #[test]
    fn patch_thread_from_message_keeps_preview_on_remove() {
        let mut thread = ThreadSummary {
            channel_id: "1".into(),
            channel_label: "t".into(),
            clan_id: "c".into(),
            parent_id: "p".into(),
            channel_private: 0,
            active: THREAD_STATUS_JOINED,
            creator_id: String::new(),
            last_message_content: "keep".into(),
            last_message_sender_id: "9".into(),
            last_message_sender_name: String::new(),
            last_message_sender_avatar: String::new(),
            last_sent_timestamp: 1,
            member_count: 0,
        };
        let msg = api::ChannelMessage {
            channel_id: 1,
            create_time_seconds: 50,
            code: 2,
            ..Default::default()
        };
        patch_thread_from_message(&mut thread, &msg);
        assert_eq!(thread.last_sent_timestamp, 50);
        assert_eq!(thread.last_message_content, "keep");
        assert_eq!(thread.last_message_sender_id, "9");
    }

    #[test]
    fn group_threads_partitions_by_status() {
        let threads = vec![
            ThreadSummary {
                channel_id: "1".into(),
                channel_label: "joined".into(),
                clan_id: "c".into(),
                parent_id: "p".into(),
                channel_private: 0,
                active: THREAD_STATUS_JOINED,
                creator_id: String::new(),
                last_message_content: String::new(),
                last_message_sender_id: String::new(),
                last_message_sender_name: String::new(),
                last_message_sender_avatar: String::new(),
                last_sent_timestamp: 3,
                member_count: 0,
            },
            ThreadSummary {
                channel_id: "2".into(),
                channel_label: "archived".into(),
                clan_id: "c".into(),
                parent_id: "p".into(),
                channel_private: 0,
                active: THREAD_STATUS_ARCHIVED,
                creator_id: String::new(),
                last_message_content: String::new(),
                last_message_sender_id: String::new(),
                last_message_sender_name: String::new(),
                last_message_sender_avatar: String::new(),
                last_sent_timestamp: 2,
                member_count: 0,
            },
            ThreadSummary {
                channel_id: "3".into(),
                channel_label: "public".into(),
                clan_id: "c".into(),
                parent_id: "p".into(),
                channel_private: 0,
                active: THREAD_STATUS_ACTIVE_PUBLIC,
                creator_id: String::new(),
                last_message_content: String::new(),
                last_message_sender_id: String::new(),
                last_message_sender_name: String::new(),
                last_message_sender_avatar: String::new(),
                last_sent_timestamp: 1,
                member_count: 0,
            },
        ];
        let grouped = group_threads(&threads);
        assert_eq!(grouped.joined, vec![0]);
        assert_eq!(grouped.active, vec![2]);
        assert_eq!(grouped.archived, vec![1]);
    }

    #[test]
    fn thread_create_fail_reason_maps_create_channel_limit() {
        use mezon_client::ApiStatusError;
        let err: anyhow::Error = ApiStatusError {
            code: ApiStatusError::OUT_OF_RANGE,
        }
        .into();
        assert_eq!(
            thread_create_fail_reason(&err),
            ThreadCreateFailReason::ChannelLimitExceeded
        );
        let err: anyhow::Error = ApiStatusError { code: 13 }.into();
        assert_eq!(
            thread_create_fail_reason(&err),
            ThreadCreateFailReason::Other
        );
    }

    #[test]
    fn channel_archive_event_uses_active_not_status() {
        let archive_active = 0i32;
        let server_status_on_archive = 1i32;
        assert_eq!(archive_active, THREAD_STATUS_ARCHIVED);
        assert_ne!(server_status_on_archive, THREAD_STATUS_ARCHIVED);
        assert_eq!(server_status_on_archive, THREAD_STATUS_JOINED);
    }

    #[gpui::test]
    fn cancel_create_clears_submitting_guard(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let api = Arc::new(mezon_client::AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            RealtimeDispatch::init(api.clone(), cx);
            ClanList::init(api.clone(), cx);
            ChannelList::init(api.clone(), cx);
            let store = ThreadsStore::init(api, cx);
            store.update(cx, |store, cx| {
                store.creating = true;
                store.submitting = true;
                store.cancel_create(cx);
                assert!(!store.is_submitting());
                assert!(!store.is_creating());
            });
        });
    }

    #[gpui::test]
    fn channel_deleted_removes_thread_and_parent_children(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let api = Arc::new(mezon_client::AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            RealtimeDispatch::init(api.clone(), cx);
            ClanList::init(api.clone(), cx);
            ChannelList::init(api.clone(), cx);
            let store = ThreadsStore::init(api, cx);
            store.update(cx, |store, cx| {
                store.threads = vec![
                    ThreadSummary {
                        channel_id: "9".into(),
                        channel_label: "t1".into(),
                        clan_id: "1".into(),
                        parent_id: "1".into(),
                        channel_private: 0,
                        active: THREAD_STATUS_JOINED,
                        creator_id: "1".into(),
                        last_message_content: String::new(),
                        last_message_sender_id: String::new(),
                        last_message_sender_name: String::new(),
                        last_message_sender_avatar: String::new(),
                        last_sent_timestamp: 0,
                        member_count: 0,
                    },
                    ThreadSummary {
                        channel_id: "10".into(),
                        channel_label: "t2".into(),
                        clan_id: "1".into(),
                        parent_id: "1".into(),
                        channel_private: 0,
                        active: THREAD_STATUS_JOINED,
                        creator_id: "1".into(),
                        last_message_content: String::new(),
                        last_message_sender_id: String::new(),
                        last_message_sender_name: String::new(),
                        last_message_sender_avatar: String::new(),
                        last_sent_timestamp: 0,
                        member_count: 0,
                    },
                    ThreadSummary {
                        channel_id: "20".into(),
                        channel_label: "other".into(),
                        clan_id: "1".into(),
                        parent_id: "2".into(),
                        channel_private: 0,
                        active: THREAD_STATUS_JOINED,
                        creator_id: "1".into(),
                        last_message_content: String::new(),
                        last_message_sender_id: String::new(),
                        last_message_sender_name: String::new(),
                        last_message_sender_avatar: String::new(),
                        last_sent_timestamp: 0,
                        member_count: 0,
                    },
                ];
                store.on_realtime_event(
                    &RealtimeEvent::ChannelDeleted(mezon_proto::realtime::ChannelDeletedEvent {
                        clan_id: 1,
                        channel_id: 9,
                        parent_id: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                assert_eq!(store.threads.len(), 2);
                assert!(!store.threads.iter().any(|t| t.channel_id == "9"));

                store.list_channel_id = Some("1".into());
                store.loaded_channel = Some("1".into());
                store.on_realtime_event(
                    &RealtimeEvent::ChannelDeleted(mezon_proto::realtime::ChannelDeletedEvent {
                        clan_id: 1,
                        channel_id: 1,
                        parent_id: 0,
                        ..Default::default()
                    }),
                    cx,
                );
                assert_eq!(store.threads.len(), 1);
                assert_eq!(store.threads[0].channel_id, "20");
                assert!(store.list_channel_id.is_none());
                assert!(store.loaded_channel.is_none());
            });
        });
    }
}
