use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::LocalBoxFuture;
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, is_unknown_api_error, mention_search_text_accepted};
use mezon_proto::api;
use unicode_normalization::UnicodeNormalization;

use crate::KeyedCache;
use crate::clan_members::{ClanMember, User};
use crate::ids::{ChannelId, ClanId, UserId};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(200);
const SERVER_RESULT_LIMIT: usize = 50;
const SESSION_RESULTS: usize = 16;

type SearchRequest = Rc<
    dyn Fn(
        MentionSearchKey,
    ) -> LocalBoxFuture<'static, anyhow::Result<api::SearchMentionUsersResponse>>,
>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MentionSearchKey {
    pub clan_id: ClanId,
    pub channel_id: Option<ChannelId>,
    pub text: String,
}

impl MentionSearchKey {
    fn same_scope(&self, clan_id: ClanId, channel_id: Option<ChannelId>) -> bool {
        self.clan_id == clan_id && self.channel_id == channel_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionSearchEvent {
    Settled,
}

struct SettledSearch {
    key: MentionSearchKey,
    text_lowercase: String,
    complete: bool,
    seq: u64,
    owner: u64,
    hits: Vec<ClanMember>,
}

impl SettledSearch {
    fn from_response(
        key: MentionSearchKey,
        seq: u64,
        owner: u64,
        response: api::SearchMentionUsersResponse,
    ) -> Self {
        Self {
            text_lowercase: key.text.to_lowercase(),
            key,
            complete: response.users.len() < SERVER_RESULT_LIMIT,
            seq,
            owner,
            hits: clan_members_from_mention_users(response.users),
        }
    }

    fn answers(&self, key: &MentionSearchKey, text_lowercase: &str) -> bool {
        self.key.same_scope(key.clan_id, key.channel_id)
            && (self.key.text == key.text
                || (self.complete && text_lowercase.starts_with(&self.text_lowercase)))
    }
}

struct InFlight {
    key: MentionSearchKey,
    owner: u64,
    session_ended: bool,
}

fn in_flight_answers(in_flight: Option<&InFlight>, key: &MentionSearchKey) -> bool {
    in_flight.is_some_and(|request| !request.session_ended && request.key == *key)
}

fn search_text(text: &str) -> String {
    text.trim().nfc().collect()
}

pub struct MentionSearchStore {
    request: SearchRequest,
    reset_generation: u64,
    owner: Option<u64>,
    debounce_pending: bool,
    last_input_at: Option<Instant>,
    last_typed: Option<String>,
    version: u64,
    wanted: Option<MentionSearchKey>,
    in_flight: Option<InFlight>,
    queued: Option<MentionSearchKey>,
    failed: Option<(u64, MentionSearchKey)>,
    results: KeyedCache<MentionSearchKey, SettledSearch>,
    latest: Option<MentionSearchKey>,
    unsupported: bool,
    _debounce_task: Task<()>,
    _request_task: Task<()>,
    _conn_watch: Task<()>,
}

struct GlobalMentionSearchStore(Entity<MentionSearchStore>);
impl Global for GlobalMentionSearchStore {}

impl EventEmitter<MentionSearchEvent> for MentionSearchStore {}

impl MentionSearchStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let request_api = api.clone();
        let request: SearchRequest = Rc::new(move |key: MentionSearchKey| {
            let api = request_api.clone();
            Box::pin(async move {
                api.search_mention_users(
                    key.clan_id.get(),
                    key.channel_id.map_or(0, |id| id.get()),
                    &key.text,
                )
                .await
            })
        });
        let entity = cx.new(|cx| Self::new(request, Self::spawn_connection_watch(api, cx)));
        cx.set_global(GlobalMentionSearchStore(entity.clone()));
        entity
    }

    fn new(request: SearchRequest, conn_watch: Task<()>) -> Self {
        Self {
            request,
            reset_generation: 0,
            owner: None,
            debounce_pending: false,
            last_input_at: None,
            last_typed: None,
            version: 0,
            wanted: None,
            in_flight: None,
            queued: None,
            failed: None,
            results: KeyedCache::new(Some(SESSION_RESULTS)),
            latest: None,
            unsupported: false,
            _debounce_task: Task::ready(()),
            _request_task: Task::ready(()),
            _conn_watch: conn_watch,
        }
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalMentionSearchStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalMentionSearchStore>()
            .map(|g| g.0.clone())
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status_rx = api.status();
            let mut was_connected = *status_rx.borrow() == ConnectionStatus::Connected;
            loop {
                if status_rx.changed().await.is_err() {
                    break;
                }
                let connected = *status_rx.borrow() == ConnectionStatus::Connected;
                if connected
                    && !was_connected
                    && this.update(cx, |this, cx| this.on_reconnected(cx)).is_err()
                {
                    break;
                }
                was_connected = connected;
            }
        })
    }

    fn on_reconnected(&mut self, cx: &mut Context<Self>) {
        self.unsupported = false;
        let Some((_, failed)) = self.failed.take() else {
            return;
        };
        if self.in_flight.is_none() && self.wanted.as_ref() == Some(&failed) {
            self.schedule(failed, cx);
        }
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self._request_task = Task::ready(());
        self.in_flight = None;
        self.last_input_at = None;
        self.unsupported = false;
        self.release_pipeline();
        self.failed = None;
        self.latest = None;
        if !self.results.is_empty() {
            self.results.clear();
            self.version = self.version.wrapping_add(1);
        }
        cx.emit(MentionSearchEvent::Settled);
        cx.notify();
    }

    pub fn end_session(&mut self, owner: u64, cx: &mut Context<Self>) {
        if let Some(request) = self.in_flight.as_mut()
            && request.owner == owner
        {
            request.session_ended = true;
        }
        let mut changed = false;
        if self.owner == Some(owner) {
            changed = self.wanted.is_some();
            self.release_pipeline();
        }
        let heir = self.owner.zip(self.wanted.clone());
        if self
            .failed
            .as_ref()
            .is_some_and(|(failed_owner, _)| *failed_owner == owner)
        {
            self.failed = None;
        }
        let owned: Vec<MentionSearchKey> = self
            .results
            .iter()
            .filter(|(_, settled)| settled.owner == owner)
            .map(|(key, _)| key.clone())
            .collect();
        let mut dropped = false;
        for key in &owned {
            let inherited = heir.as_ref().is_some_and(|(heir, wanted)| {
                let text_lowercase = wanted.text.to_lowercase();
                self.results.get_mut(key).is_some_and(|settled| {
                    let answers = settled.answers(wanted, &text_lowercase);
                    if answers {
                        settled.owner = *heir;
                    }
                    answers
                })
            });
            if !inherited {
                self.results.remove(key);
                dropped = true;
            }
        }
        if dropped {
            self.version = self.version.wrapping_add(1);
            changed = true;
        }
        if self
            .latest
            .as_ref()
            .is_some_and(|latest| self.results.get(latest).is_none())
        {
            self.latest = None;
        }
        if changed {
            cx.emit(MentionSearchEvent::Settled);
            cx.notify();
        }
    }

    fn release_pipeline(&mut self) {
        self.owner = None;
        self.last_typed = None;
        self.cancel_debounce();
        self.wanted = None;
        self.queued = None;
    }

    fn failed_on(&self, key: &MentionSearchKey) -> bool {
        self.failed
            .as_ref()
            .is_some_and(|(_, failed)| failed == key)
    }

    fn answered(&self, key: &MentionSearchKey) -> bool {
        let text_lowercase = key.text.to_lowercase();
        self.results
            .iter()
            .any(|(_, settled)| settled.answers(key, &text_lowercase))
    }

    fn best_for(&self, key: &MentionSearchKey) -> Option<&SettledSearch> {
        self.results.get(key).or_else(|| {
            let text_lowercase = key.text.to_lowercase();
            self.results
                .iter()
                .map(|(_, settled)| settled)
                .filter(|settled| settled.answers(key, &text_lowercase))
                .max_by_key(|settled| settled.key.text.len())
        })
    }

    pub fn is_searching(
        &self,
        owner: u64,
        clan_id: ClanId,
        channel_id: Option<ChannelId>,
        text: &str,
    ) -> bool {
        let Some(wanted) = self.wanted.as_ref() else {
            return false;
        };
        self.owner == Some(owner)
            && wanted.same_scope(clan_id, channel_id)
            && wanted.text == search_text(text)
            && !self.answered(wanted)
            && !self.failed_on(wanted)
            && (self.debounce_pending || self.in_flight.is_some() || self.queued.is_some())
    }

    pub fn results(
        &self,
        clan_id: ClanId,
        channel_id: Option<ChannelId>,
        text: &str,
    ) -> Option<(&str, u64, &[ClanMember])> {
        let key = MentionSearchKey {
            clan_id,
            channel_id,
            text: search_text(text),
        };
        self.best_for(&key)
            .or_else(|| {
                self.latest
                    .as_ref()
                    .filter(|latest| latest.same_scope(clan_id, channel_id))
                    .and_then(|latest| self.results.get(latest))
            })
            .map(|settled| {
                (
                    settled.key.text.as_str(),
                    settled.seq,
                    settled.hits.as_slice(),
                )
            })
    }

    pub fn answer_complete(
        &self,
        clan_id: ClanId,
        channel_id: Option<ChannelId>,
        text: &str,
    ) -> Option<bool> {
        let key = MentionSearchKey {
            clan_id,
            channel_id,
            text: search_text(text),
        };
        self.best_for(&key).map(|settled| settled.complete)
    }

    pub fn search(
        &mut self,
        owner: u64,
        clan_id: ClanId,
        channel_id: Option<ChannelId>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let taken_over = self
            .owner
            .replace(owner)
            .is_some_and(|previous| previous != owner);
        if taken_over {
            self.last_typed = None;
        }
        self.search_for_owner(clan_id, channel_id, text, cx);
        if taken_over {
            cx.emit(MentionSearchEvent::Settled);
        }
    }

    fn search_for_owner(
        &mut self,
        clan_id: ClanId,
        channel_id: Option<ChannelId>,
        typed: &str,
        cx: &mut Context<Self>,
    ) {
        let retyped = self.last_typed.as_deref() != Some(typed);
        if retyped {
            self.last_typed = Some(typed.to_string());
        }
        let text = search_text(typed);
        if self.unsupported || !mention_search_text_accepted(&text) {
            self.wanted = None;
            self.queued = None;
            self.cancel_debounce();
            return;
        }
        let key = MentionSearchKey {
            clan_id,
            channel_id,
            text,
        };
        let same_as_wanted = self.wanted.as_ref() == Some(&key);
        if retyped || !same_as_wanted {
            self.last_input_at = Some(cx.background_executor().now());
        }
        self.wanted = Some(key.clone());
        if self.answered(&key)
            || self.failed_on(&key)
            || in_flight_answers(self.in_flight.as_ref(), &key)
        {
            self.queued = None;
            self.cancel_debounce();
            return;
        }
        if self.in_flight.is_some() {
            self.queued = Some(key);
            return;
        }
        if same_as_wanted && self.debounce_pending && !retyped {
            return;
        }
        self.schedule(key, cx);
    }

    fn cancel_debounce(&mut self) {
        self.debounce_pending = false;
        self._debounce_task = Task::ready(());
    }

    fn schedule(&mut self, key: MentionSearchKey, cx: &mut Context<Self>) {
        self.cancel_debounce();
        self.debounce_pending = true;
        let now = cx.background_executor().now();
        let delay = self.last_input_at.map_or(SEARCH_DEBOUNCE, |at| {
            SEARCH_DEBOUNCE.saturating_sub(now.saturating_duration_since(at))
        });
        self._debounce_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                this.debounce_pending = false;
                if this.in_flight.is_none() {
                    this.send(key, cx);
                } else {
                    this.queued = Some(key);
                }
            });
        });
    }

    fn send(&mut self, key: MentionSearchKey, cx: &mut Context<Self>) {
        let owner = self.owner.unwrap_or_default();
        let reset_generation = self.reset_generation;
        self.in_flight = Some(InFlight {
            key: key.clone(),
            owner,
            session_ended: false,
        });
        let request = (self.request)(key.clone());
        self._request_task = cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                this.complete(reset_generation, owner, key, result, cx)
            });
        });
    }

    fn complete(
        &mut self,
        reset_generation: u64,
        owner: u64,
        key: MentionSearchKey,
        result: anyhow::Result<api::SearchMentionUsersResponse>,
        cx: &mut Context<Self>,
    ) {
        if self.reset_generation != reset_generation {
            return;
        }
        let session_live = self
            .in_flight
            .take()
            .is_some_and(|request| !request.session_ended);
        if session_live {
            match result {
                Ok(response) => {
                    self.version = self.version.wrapping_add(1);
                    let fresh =
                        SettledSearch::from_response(key.clone(), self.version, owner, response);
                    let in_use = self
                        .wanted
                        .as_ref()
                        .and_then(|wanted| self.best_for(wanted))
                        .map(|settled| settled.key.clone());
                    let protect: Vec<&MentionSearchKey> =
                        self.wanted.iter().chain(in_use.iter()).collect();
                    self.results.insert_protecting(key.clone(), fresh, &protect);
                    if self.failed_on(&key) {
                        self.failed = None;
                    }
                    self.latest = Some(key);
                }
                Err(err) if is_unknown_api_error(&err) => {
                    tracing::info!(
                        "SearchMentionUsers is not served by this socket server; using the local roster until reconnect"
                    );
                    self.unsupported = true;
                    self.wanted = None;
                    self.queued = None;
                }
                Err(err) => {
                    tracing::warn!("SearchMentionUsers failed: {err}");
                    self.failed = Some((owner, key));
                }
            }
        }
        if let Some(next) = self.queued.take().or_else(|| self.wanted.clone())
            && !self.answered(&next)
            && !self.failed_on(&next)
        {
            self.schedule(next, cx);
        }
        cx.emit(MentionSearchEvent::Settled);
        cx.notify();
    }
}

fn clan_members_from_mention_users(users: Vec<api::MentionUser>) -> Vec<ClanMember> {
    users
        .into_iter()
        .filter(|user| user.id != 0)
        .map(|user| ClanMember {
            user: User {
                id: UserId(user.id),
                username: user.username,
                display_name: user.display_name,
                avatar_url: user.avatar_url,
                ..User::default()
            },
            clan_nick: user.clan_nick,
            clan_avatar: user.clan_avatar,
            ..ClanMember::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use futures::channel::oneshot;
    use gpui::TestAppContext;

    type Reply = oneshot::Sender<anyhow::Result<api::SearchMentionUsersResponse>>;

    #[derive(Default)]
    struct FakeServer {
        sent: Vec<String>,
        replies: VecDeque<Reply>,
    }

    impl FakeServer {
        fn answer(&mut self, users: Vec<api::MentionUser>) {
            let reply = self.replies.pop_front().expect("a request is waiting");
            let _ = reply.send(Ok(api::SearchMentionUsersResponse { users }));
        }

        fn fail_unknown(&mut self) {
            let reply = self.replies.pop_front().expect("a request is waiting");
            let _ = reply.send(Err(mezon_client::unknown_api_error("SearchMentionUsers")));
        }
    }

    fn fake_store(
        cx: &mut TestAppContext,
    ) -> (Entity<MentionSearchStore>, Rc<RefCell<FakeServer>>) {
        let server = Rc::new(RefCell::new(FakeServer::default()));
        let handle = server.clone();
        let request: SearchRequest = Rc::new(move |key: MentionSearchKey| {
            let (reply, answer) = oneshot::channel();
            let mut server = handle.borrow_mut();
            server.sent.push(key.text);
            server.replies.push_back(reply);
            Box::pin(async move {
                answer
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("dropped")))
            })
        });
        let store = cx.update(|cx| cx.new(|_| MentionSearchStore::new(request, Task::ready(()))));
        (store, server)
    }

    const COMPOSER: u64 = 1;
    const OTHER_COMPOSER: u64 = 2;

    fn type_query(store: &Entity<MentionSearchStore>, text: &str, cx: &mut TestAppContext) {
        store.update(cx, |store, cx| {
            store.search(COMPOSER, ClanId(1), None, text, cx)
        });
    }

    fn advance(ms: u64, cx: &mut TestAppContext) {
        cx.executor().advance_clock(Duration::from_millis(ms));
        cx.run_until_parked();
    }

    fn sent(server: &Rc<RefCell<FakeServer>>) -> Vec<String> {
        server.borrow().sent.clone()
    }

    fn shown(store: &MentionSearchStore, clan: i64, text: &str) -> (String, usize) {
        store
            .results(ClanId(clan), None, text)
            .map_or((String::new(), 0), |(from, _, hits)| {
                (from.to_string(), hits.len())
            })
    }

    fn mention_user(id: i64, username: &str, clan_nick: &str) -> api::MentionUser {
        api::MentionUser {
            id,
            username: username.to_string(),
            display_name: format!("{username} display"),
            avatar_url: format!("https://cdn/{username}.png"),
            clan_nick: clan_nick.to_string(),
            clan_avatar: String::new(),
        }
    }

    fn key(clan: i64, channel: Option<i64>, text: &str) -> MentionSearchKey {
        MentionSearchKey {
            clan_id: ClanId(clan),
            channel_id: channel.map(ChannelId),
            text: text.to_string(),
        }
    }

    fn settled(text: &str, users: Vec<api::MentionUser>) -> SettledSearch {
        SettledSearch::from_response(
            key(1, None, text),
            0,
            COMPOSER,
            api::SearchMentionUsersResponse { users },
        )
    }

    fn answers(search: &SettledSearch, key: &MentionSearchKey) -> bool {
        search.answers(key, &key.text.to_lowercase())
    }

    fn many(count: usize) -> Vec<api::MentionUser> {
        (0..count)
            .map(|i| mention_user(i as i64 + 1, &format!("ng{i}"), ""))
            .collect()
    }

    #[test]
    fn mention_users_map_to_clan_members_with_clan_profile() {
        let members = clan_members_from_mention_users(vec![
            mention_user(7, "alice", "Boss"),
            mention_user(8, "bob", ""),
        ]);
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].id(), UserId(7));
        assert_eq!(members[0].name(), "Boss");
        assert_eq!(members[0].avatar(), "https://cdn/alice.png");
        assert_eq!(members[1].name(), "bob display");
        assert_eq!(members[1].user.username, "bob");
    }

    #[test]
    fn mention_users_without_id_are_dropped() {
        let members = clan_members_from_mention_users(vec![
            mention_user(0, "ghost", ""),
            mention_user(9, "carol", ""),
        ]);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id(), UserId(9));
    }

    #[test]
    fn a_complete_result_answers_only_queries_that_extend_it() {
        let search = settled("ng", many(12));
        assert!(answers(&search, &key(1, None, "ng")));
        assert!(answers(&search, &key(1, None, "nguyen")));
        assert!(answers(&search, &key(1, None, "NGU")));
        assert!(!answers(&search, &key(1, None, "hoang")));
        assert!(!answers(&search, &key(1, None, "n")));
        assert!(!answers(&search, &key(2, None, "nguyen")));
        assert!(!answers(&search, &key(1, Some(5), "nguyen")));
    }

    #[test]
    fn a_result_cut_at_the_server_limit_only_answers_its_own_query() {
        let search = settled("ng", many(SERVER_RESULT_LIMIT));
        assert!(answers(&search, &key(1, None, "ng")));
        assert!(!answers(&search, &key(1, None, "nguyen")));
    }

    #[test]
    fn completeness_counts_the_rows_the_server_sent() {
        let mut users = many(SERVER_RESULT_LIMIT - 1);
        users.push(mention_user(0, "ghost", ""));
        let search = settled("ng", users);
        assert_eq!(search.hits.len(), SERVER_RESULT_LIMIT - 1);
        assert!(!search.complete);
    }

    #[test]
    fn only_the_current_session_s_request_answers_a_repeated_query() {
        let mut pending = InFlight {
            key: key(1, None, "ng"),
            owner: COMPOSER,
            session_ended: false,
        };
        assert!(in_flight_answers(Some(&pending), &key(1, None, "ng")));
        assert!(!in_flight_answers(Some(&pending), &key(1, None, "ngu")));
        assert!(!in_flight_answers(None, &key(1, None, "ng")));
        pending.session_ended = true;
        assert!(!in_flight_answers(Some(&pending), &key(1, None, "ng")));
    }

    #[gpui::test]
    fn a_query_is_sent_once_typing_pauses(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(150, cx);
        type_query(&store, "ngu", cx);
        advance(150, cx);
        assert!(sent(&server).is_empty());
        advance(60, cx);
        assert_eq!(sent(&server), vec!["ngu"]);
    }

    #[gpui::test]
    fn repeating_the_waiting_query_does_not_restart_the_debounce(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(150, cx);
        type_query(&store, "ng", cx);
        advance(60, cx);
        assert_eq!(sent(&server), vec!["ng"]);
    }

    #[gpui::test]
    fn one_request_at_a_time_and_the_newest_query_follows(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(200, cx);
        type_query(&store, "ngu", cx);
        type_query(&store, "nguy", cx);
        advance(300, cx);
        assert_eq!(sent(&server), vec!["ng"]);
        server.borrow_mut().answer(many(SERVER_RESULT_LIMIT));
        cx.run_until_parked();
        assert_eq!(sent(&server), vec!["ng", "nguy"]);
        assert!(store.read_with(cx, |store, _| store.is_searching(
            COMPOSER,
            ClanId(1),
            None,
            "nguy"
        )));
    }

    #[gpui::test]
    fn a_complete_result_answers_longer_queries_without_a_request(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "bo", cx);
        advance(200, cx);
        server
            .borrow_mut()
            .answer(vec![mention_user(7, "alice", "Boss")]);
        cx.run_until_parked();
        type_query(&store, "bos", cx);
        type_query(&store, "boss", cx);
        advance(400, cx);
        assert_eq!(sent(&server), vec!["bo"]);
        store.read_with(cx, |store, _| {
            assert!(!store.is_searching(COMPOSER, ClanId(1), None, "boss"));
            assert_eq!(shown(store, 1, "boss"), ("bo".to_string(), 1));
            assert_eq!(shown(store, 2, "boss").1, 0);
        });
    }

    #[gpui::test]
    fn a_new_session_resends_a_query_whose_request_belonged_to_the_last_one(
        cx: &mut TestAppContext,
    ) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(200, cx);
        store.update(cx, |store, cx| store.end_session(COMPOSER, cx));
        type_query(&store, "ng", cx);
        server.borrow_mut().answer(many(3));
        cx.run_until_parked();
        assert!(store.read_with(cx, |store, _| shown(store, 1, "ng").1 == 0));
        advance(200, cx);
        assert_eq!(sent(&server), vec!["ng", "ng"]);
        server.borrow_mut().answer(many(3));
        cx.run_until_parked();
        assert_eq!(store.read_with(cx, |store, _| shown(store, 1, "ng").1), 3);
    }

    #[gpui::test]
    fn a_short_query_drops_the_queued_one(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(200, cx);
        type_query(&store, "ngu", cx);
        type_query(&store, "n", cx);
        server.borrow_mut().answer(many(SERVER_RESULT_LIMIT));
        advance(400, cx);
        assert_eq!(sent(&server), vec!["ng"]);
    }

    #[gpui::test]
    fn an_unknown_api_turns_the_search_off_until_reconnect(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(200, cx);
        server.borrow_mut().fail_unknown();
        cx.run_until_parked();
        type_query(&store, "ngu", cx);
        advance(400, cx);
        assert_eq!(sent(&server), vec!["ng"]);
        assert!(!store.read_with(cx, |store, _| store.is_searching(
            COMPOSER,
            ClanId(1),
            None,
            "ngu"
        )));
        store.update(cx, |store, cx| store.on_reconnected(cx));
        type_query(&store, "nguy", cx);
        advance(200, cx);
        assert_eq!(sent(&server), vec!["ng", "nguy"]);
    }

    #[gpui::test]
    fn logout_forgets_results_and_waiting_queries(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "bo", cx);
        advance(200, cx);
        server
            .borrow_mut()
            .answer(vec![mention_user(7, "alice", "Boss")]);
        cx.run_until_parked();
        type_query(&store, "tr", cx);
        store.update(cx, |store, cx| store.reset(cx));
        advance(400, cx);
        assert_eq!(sent(&server), vec!["bo"]);
        store.read_with(cx, |store, _| {
            assert_eq!(shown(store, 1, "bo").1, 0);
            assert!(!store.is_searching(COMPOSER, ClanId(1), None, "tr"));
        });
    }

    #[gpui::test]
    fn another_composer_cannot_end_the_session_it_does_not_own(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "bo", cx);
        advance(200, cx);
        server
            .borrow_mut()
            .answer(vec![mention_user(7, "alice", "Boss")]);
        cx.run_until_parked();
        store.update(cx, |store, cx| store.end_session(OTHER_COMPOSER, cx));
        assert_eq!(store.read_with(cx, |store, _| shown(store, 1, "bo").1), 1);
        store.update(cx, |store, cx| store.end_session(COMPOSER, cx));
        assert!(store.read_with(cx, |store, _| shown(store, 1, "bo").1 == 0));
    }

    #[gpui::test]
    fn backspacing_to_an_answered_query_reuses_its_result(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(200, cx);
        server.borrow_mut().answer(many(SERVER_RESULT_LIMIT));
        cx.run_until_parked();
        type_query(&store, "ngu", cx);
        advance(200, cx);
        server.borrow_mut().answer(many(5));
        cx.run_until_parked();
        type_query(&store, "ng", cx);
        advance(400, cx);
        assert_eq!(sent(&server), vec!["ng", "ngu"]);
        store.read_with(cx, |store, _| {
            assert!(!store.is_searching(COMPOSER, ClanId(1), None, "ng"));
            assert_eq!(
                shown(store, 1, "ng"),
                ("ng".to_string(), SERVER_RESULT_LIMIT)
            );
            assert_eq!(shown(store, 1, "nguy"), ("ngu".to_string(), 5));
        });
    }

    #[gpui::test]
    fn a_late_answer_does_not_take_over_a_query_answered_by_another(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(200, cx);
        server.borrow_mut().answer(many(12));
        cx.run_until_parked();
        type_query(&store, "tr", cx);
        advance(200, cx);
        type_query(&store, "ngu", cx);
        server.borrow_mut().answer(many(3));
        cx.run_until_parked();
        assert_eq!(sent(&server), vec!["ng", "tr"]);
        store.read_with(cx, |store, _| {
            assert_eq!(shown(store, 1, "ngu"), ("ng".to_string(), 12));
            assert_eq!(shown(store, 1, "tra"), ("tr".to_string(), 3));
        });
    }

    #[gpui::test]
    fn a_failed_query_is_not_sent_again_until_the_text_changes(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(200, cx);
        let reply = server
            .borrow_mut()
            .replies
            .pop_front()
            .expect("a request is waiting");
        let _ = reply.send(Err(anyhow::anyhow!("API error: code=13")));
        cx.run_until_parked();
        type_query(&store, "ng", cx);
        advance(400, cx);
        assert_eq!(sent(&server), vec!["ng"]);
        assert!(!store.read_with(cx, |store, _| store.is_searching(
            COMPOSER,
            ClanId(1),
            None,
            "ng"
        )));
        type_query(&store, "ngu", cx);
        advance(200, cx);
        assert_eq!(sent(&server), vec!["ng", "ngu"]);
    }

    #[gpui::test]
    fn ending_a_session_tells_every_composer(cx: &mut TestAppContext) {
        let (store, _server) = fake_store(cx);
        let settled = Rc::new(std::cell::Cell::new(0));
        let seen = settled.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&store, move |_, _: &MentionSearchEvent, _| {
                seen.set(seen.get() + 1)
            })
        });
        type_query(&store, "ng", cx);
        store.update(cx, |store, cx| store.end_session(COMPOSER, cx));
        cx.run_until_parked();
        assert_eq!(settled.get(), 1);
    }

    fn type_query_as(
        store: &Entity<MentionSearchStore>,
        owner: u64,
        text: &str,
        cx: &mut TestAppContext,
    ) {
        store.update(cx, |store, cx| {
            store.search(owner, ClanId(1), None, text, cx)
        });
    }

    #[gpui::test]
    fn ending_one_composer_s_session_keeps_another_composer_s_results(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query_as(&store, COMPOSER, "bo", cx);
        advance(200, cx);
        server
            .borrow_mut()
            .answer(vec![mention_user(7, "alice", "Boss")]);
        cx.run_until_parked();
        type_query_as(&store, OTHER_COMPOSER, "tr", cx);
        advance(200, cx);
        server.borrow_mut().answer(many(3));
        cx.run_until_parked();
        store.update(cx, |store, cx| store.end_session(OTHER_COMPOSER, cx));
        store.read_with(cx, |store, _| {
            assert_eq!(shown(store, 1, "bo"), ("bo".to_string(), 1));
            assert_eq!(shown(store, 1, "tra").1, 0);
        });
    }

    #[gpui::test]
    fn logout_tells_every_composer(cx: &mut TestAppContext) {
        let (store, _server) = fake_store(cx);
        let settled = Rc::new(std::cell::Cell::new(0));
        let seen = settled.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&store, move |_, _: &MentionSearchEvent, _| {
                seen.set(seen.get() + 1)
            })
        });
        type_query(&store, "ng", cx);
        store.update(cx, |store, cx| store.reset(cx));
        cx.run_until_parked();
        assert_eq!(settled.get(), 1);
    }

    fn fail_next(server: &Rc<RefCell<FakeServer>>, message: &str) {
        let reply = server
            .borrow_mut()
            .replies
            .pop_front()
            .expect("a request is waiting");
        let _ = reply.send(Err(anyhow::anyhow!(message.to_string())));
    }

    fn count_settled(
        store: &Entity<MentionSearchStore>,
        cx: &mut TestAppContext,
    ) -> (Rc<std::cell::Cell<usize>>, gpui::Subscription) {
        let settled = Rc::new(std::cell::Cell::new(0));
        let seen = settled.clone();
        let subscription = cx.update(|cx| {
            cx.subscribe(store, move |_, _: &MentionSearchEvent, _| {
                seen.set(seen.get() + 1)
            })
        });
        (settled, subscription)
    }

    #[gpui::test]
    fn a_query_that_failed_while_offline_is_sent_again_on_reconnect(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(200, cx);
        fail_next(&server, "not connected");
        cx.run_until_parked();
        store.update(cx, |store, cx| store.on_reconnected(cx));
        advance(200, cx);
        assert_eq!(sent(&server), vec!["ng", "ng"]);
        server.borrow_mut().answer(many(3));
        cx.run_until_parked();
        assert_eq!(store.read_with(cx, |store, _| shown(store, 1, "ng").1), 3);
    }

    #[gpui::test]
    fn a_late_answer_never_evicts_the_result_in_use(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(200, cx);
        server.borrow_mut().answer(many(12));
        cx.run_until_parked();
        for i in 0..SESSION_RESULTS - 1 {
            type_query(&store, &format!("q{i:02}"), cx);
            advance(200, cx);
            server.borrow_mut().answer(many(SERVER_RESULT_LIMIT));
            cx.run_until_parked();
        }
        type_query(&store, "tr", cx);
        advance(200, cx);
        type_query(&store, "ngu", cx);
        server.borrow_mut().answer(many(3));
        cx.run_until_parked();
        store.read_with(cx, |store, _| {
            assert_eq!(shown(store, 1, "ngu"), ("ng".to_string(), 12));
            assert!(!store.is_searching(COMPOSER, ClanId(1), None, "ngu"));
        });
    }

    #[gpui::test]
    fn a_composer_waiting_on_an_ended_session_s_request_gets_its_own(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query_as(&store, COMPOSER, "ng", cx);
        advance(200, cx);
        type_query_as(&store, OTHER_COMPOSER, "ng", cx);
        store.update(cx, |store, cx| store.end_session(COMPOSER, cx));
        server.borrow_mut().answer(many(3));
        cx.run_until_parked();
        assert!(store.read_with(cx, |store, _| {
            store.is_searching(OTHER_COMPOSER, ClanId(1), None, "ng")
        }));
        advance(200, cx);
        assert_eq!(sent(&server), vec!["ng", "ng"]);
        server.borrow_mut().answer(many(3));
        cx.run_until_parked();
        assert_eq!(store.read_with(cx, |store, _| shown(store, 1, "ng").1), 3);
    }

    #[gpui::test]
    fn a_composer_taking_over_the_search_tells_the_others(cx: &mut TestAppContext) {
        let (store, _server) = fake_store(cx);
        let (settled, _subscription) = count_settled(&store, cx);
        type_query_as(&store, COMPOSER, "ng", cx);
        type_query_as(&store, COMPOSER, "ngu", cx);
        cx.run_until_parked();
        assert_eq!(settled.get(), 0);
        type_query_as(&store, OTHER_COMPOSER, "tr", cx);
        cx.run_until_parked();
        assert_eq!(settled.get(), 1);
        assert!(!store.read_with(cx, |store, _| store.is_searching(
            COMPOSER,
            ClanId(1),
            None,
            "ngu"
        )));
    }

    #[gpui::test]
    fn ending_a_session_with_nothing_to_drop_tells_nobody(cx: &mut TestAppContext) {
        let (store, _server) = fake_store(cx);
        let (settled, _subscription) = count_settled(&store, cx);
        store.update(cx, |store, cx| store.end_session(COMPOSER, cx));
        cx.run_until_parked();
        assert_eq!(settled.get(), 0);
    }

    #[gpui::test]
    fn a_cut_answer_is_reported_for_the_queries_it_answers(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "kha", cx);
        advance(200, cx);
        server.borrow_mut().answer(many(SERVER_RESULT_LIMIT));
        cx.run_until_parked();
        type_query(&store, "bo", cx);
        advance(200, cx);
        server.borrow_mut().answer(many(2));
        cx.run_until_parked();
        store.read_with(cx, |store, _| {
            assert_eq!(store.answer_complete(ClanId(1), None, "kha "), Some(false));
            assert_eq!(store.answer_complete(ClanId(1), None, "bo "), Some(true));
            assert_eq!(store.answer_complete(ClanId(1), None, "bob"), Some(true));
            assert_eq!(store.answer_complete(ClanId(1), None, "tr"), None);
        });
    }

    #[gpui::test]
    fn decomposed_text_is_sent_composed(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "Nguye\u{0302}\u{0303}n", cx);
        advance(200, cx);
        assert_eq!(sent(&server), vec!["Nguy\u{1ec5}n"]);
        assert!(store.read_with(cx, |store, _| {
            store.is_searching(COMPOSER, ClanId(1), None, "Nguye\u{0302}\u{0303}n")
        }));
    }

    #[gpui::test]
    fn ending_a_session_keeps_the_result_another_composer_is_reading(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query_as(&store, COMPOSER, "ng", cx);
        advance(200, cx);
        server.borrow_mut().answer(many(3));
        cx.run_until_parked();
        type_query_as(&store, OTHER_COMPOSER, "ngu", cx);
        store.update(cx, |store, cx| store.end_session(COMPOSER, cx));
        advance(400, cx);
        assert_eq!(sent(&server), vec!["ng"]);
        store.read_with(cx, |store, _| {
            assert_eq!(shown(store, 1, "ngu"), ("ng".to_string(), 3));
            assert!(!store.is_searching(OTHER_COMPOSER, ClanId(1), None, "ngu"));
        });
        store.update(cx, |store, cx| store.end_session(OTHER_COMPOSER, cx));
        assert_eq!(store.read_with(cx, |store, _| shown(store, 1, "ngu").1), 0);
    }

    #[gpui::test]
    fn an_answer_to_a_request_sent_before_logout_never_lands(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "ng", cx);
        advance(200, cx);
        store.update(cx, |store, cx| store.reset(cx));
        server.borrow_mut().answer(many(3));
        advance(400, cx);
        assert_eq!(sent(&server), vec!["ng"]);
        store.read_with(cx, |store, _| {
            assert_eq!(shown(store, 1, "ng").1, 0);
            assert!(!store.is_searching(COMPOSER, ClanId(1), None, "ng"));
        });
        type_query(&store, "ng", cx);
        advance(200, cx);
        assert_eq!(sent(&server), vec!["ng", "ng"]);
        server.borrow_mut().answer(many(2));
        cx.run_until_parked();
        assert_eq!(store.read_with(cx, |store, _| shown(store, 1, "ng").1), 2);
    }

    #[gpui::test]
    fn a_typed_space_counts_as_typing(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query(&store, "old", cx);
        advance(150, cx);
        type_query(&store, "old ", cx);
        advance(150, cx);
        assert!(sent(&server).is_empty());
        type_query(&store, "old t", cx);
        advance(150, cx);
        assert!(sent(&server).is_empty());
        advance(60, cx);
        assert_eq!(sent(&server), vec!["old t"]);
    }

    #[gpui::test]
    fn only_the_composer_driving_the_search_is_searching(cx: &mut TestAppContext) {
        let (store, _server) = fake_store(cx);
        let searching = |owner: u64, text: &str, cx: &mut TestAppContext| {
            store.read_with(cx, |store, _| {
                store.is_searching(owner, ClanId(1), None, text)
            })
        };
        type_query_as(&store, COMPOSER, "ng", cx);
        assert!(searching(COMPOSER, "ng", cx));
        type_query_as(&store, OTHER_COMPOSER, "ng", cx);
        assert!(!searching(COMPOSER, "ng", cx));
        assert!(searching(OTHER_COMPOSER, "ng", cx));
        type_query_as(&store, OTHER_COMPOSER, "n", cx);
        assert!(!searching(COMPOSER, "ng", cx));
        assert!(!searching(OTHER_COMPOSER, "n", cx));
    }

    #[gpui::test]
    fn a_composer_taking_over_waits_its_own_debounce(cx: &mut TestAppContext) {
        let (store, server) = fake_store(cx);
        type_query_as(&store, COMPOSER, "ng", cx);
        advance(150, cx);
        type_query_as(&store, OTHER_COMPOSER, "ng", cx);
        advance(100, cx);
        assert!(sent(&server).is_empty());
        advance(110, cx);
        assert_eq!(sent(&server), vec!["ng"]);
    }
}
