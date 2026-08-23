use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, Global, Task};
use mezon_client::{AppApi, ConnectionStatus};

use crate::clan::OnboardingItem;
use crate::ids::ClanId;

const CACHE_TTL: Duration = Duration::from_secs(20 * 60);
const FETCH_RETRY_BACKOFF: Duration = Duration::from_secs(30);
const ONBOARDING_PAGE_LIMIT: i32 = 100;

pub const GUIDE_TYPE_GREETING: i32 = 1;
pub const GUIDE_TYPE_RULE: i32 = 2;
pub const GUIDE_TYPE_TASK: i32 = 3;
pub const GUIDE_TYPE_QUESTION: i32 = 4;

pub const MISSION_SEND_MESSAGE: i32 = 1;
pub const MISSION_VISIT: i32 = 2;
pub const MISSION_DO_SOMETHING: i32 = 3;

pub const DONE_ONBOARDING_STATUS: i32 = 3;

#[derive(Debug, Clone, Default)]
pub struct ClanOnboarding {
    pub greeting: Option<OnboardingItem>,
    pub rules: Vec<OnboardingItem>,
    pub questions: Vec<OnboardingItem>,
    pub missions: Vec<OnboardingItem>,
}

impl ClanOnboarding {
    pub fn from_items(items: Vec<OnboardingItem>) -> Self {
        let mut onboarding = Self::default();
        for item in items {
            match item.guide_type {
                GUIDE_TYPE_GREETING => onboarding.greeting = Some(item),
                GUIDE_TYPE_RULE => onboarding.rules.push(item),
                GUIDE_TYPE_QUESTION => onboarding.questions.push(item),
                GUIDE_TYPE_TASK => onboarding.missions.push(item),
                _ => {}
            }
        }
        onboarding
    }

    pub fn answer_count(&self) -> usize {
        self.questions
            .iter()
            .map(|question| question.answers.len())
            .sum()
    }
}

pub struct OnboardingStore {
    clans: HashMap<ClanId, ClanOnboarding>,
    loading: HashSet<ClanId>,
    fetched_at: HashMap<ClanId, Instant>,
    failed_at: HashMap<ClanId, Instant>,
    steps: HashMap<ClanId, i32>,
    steps_loading: HashSet<ClanId>,
    steps_fetched_at: HashMap<ClanId, Instant>,
    steps_failed_at: HashMap<ClanId, Instant>,
    mission_done: HashMap<ClanId, usize>,
    answers: HashMap<ClanId, HashSet<(i64, usize)>>,
    reset_generation: u64,
    api: Arc<AppApi>,
    _connection_watch: Task<()>,
}

struct GlobalOnboardingStore(Entity<OnboardingStore>);
impl Global for GlobalOnboardingStore {}

impl OnboardingStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalOnboardingStore(entity.clone()));
        entity
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let connection_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            clans: HashMap::new(),
            loading: HashSet::new(),
            fetched_at: HashMap::new(),
            failed_at: HashMap::new(),
            steps: HashMap::new(),
            steps_loading: HashSet::new(),
            steps_fetched_at: HashMap::new(),
            steps_failed_at: HashMap::new(),
            mission_done: HashMap::new(),
            answers: HashMap::new(),
            reset_generation: 0,
            api,
            _connection_watch: connection_watch,
        }
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status = api.status();
            let mut connected_before = *status.borrow() == ConnectionStatus::Connected;
            loop {
                if status.changed().await.is_err() {
                    break;
                }
                let connected = *status.borrow() == ConnectionStatus::Connected;
                if connected
                    && !connected_before
                    && this.update(cx, |this, cx| this.retry_failed(cx)).is_err()
                {
                    break;
                }
                connected_before = connected;
            }
        })
    }

    fn retry_failed(&mut self, cx: &mut Context<Self>) {
        let clans: HashSet<ClanId> = self
            .failed_at
            .keys()
            .copied()
            .chain(self.steps_failed_at.keys().copied())
            .collect();
        if clans.is_empty() {
            return;
        }
        for clan_id in clans {
            self.refetch(clan_id, cx);
        }
        cx.notify();
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalOnboardingStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalOnboardingStore>()
            .map(|store| store.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.clans.clear();
        self.loading.clear();
        self.fetched_at.clear();
        self.failed_at.clear();
        self.steps.clear();
        self.steps_loading.clear();
        self.steps_fetched_at.clear();
        self.steps_failed_at.clear();
        self.mission_done.clear();
        self.answers.clear();
        self.reset_generation = self.reset_generation.wrapping_add(1);
        cx.notify();
    }

    pub fn onboarding(&self, clan_id: ClanId) -> Option<&ClanOnboarding> {
        self.clans.get(&clan_id)
    }

    pub fn is_loading(&self, clan_id: ClanId) -> bool {
        self.loading.contains(&clan_id)
    }

    pub fn load_failed(&self, clan_id: ClanId) -> bool {
        !self.clans.contains_key(&clan_id) && self.failed_at.contains_key(&clan_id)
    }

    pub fn has_onboarding(&self, clan_id: ClanId) -> bool {
        self.clans.contains_key(&clan_id)
    }

    pub fn mission_done(&self, clan_id: ClanId) -> usize {
        if self.is_finished(clan_id) {
            return self
                .clans
                .get(&clan_id)
                .map_or(0, |onboarding| onboarding.missions.len());
        }
        self.mission_done.get(&clan_id).copied().unwrap_or(0)
    }

    pub fn is_finished(&self, clan_id: ClanId) -> bool {
        self.steps.get(&clan_id).copied() == Some(DONE_ONBOARDING_STATUS)
    }

    pub fn answer_selected(&self, clan_id: ClanId, question_id: i64, index: usize) -> bool {
        self.answers
            .get(&clan_id)
            .is_some_and(|answers| answers.contains(&(question_id, index)))
    }

    pub fn answered_count(&self, clan_id: ClanId) -> usize {
        self.answers.get(&clan_id).map_or(0, HashSet::len)
    }

    pub fn answered_percent(&self, clan_id: ClanId) -> f32 {
        let total = self
            .clans
            .get(&clan_id)
            .map_or(0, ClanOnboarding::answer_count);
        if total == 0 {
            return 0.;
        }
        (self.answered_count(clan_id) as f32 * 100.) / total as f32
    }

    pub fn toggle_answer(
        &mut self,
        clan_id: ClanId,
        question_id: i64,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let answers = self.answers.entry(clan_id).or_default();
        if !answers.insert((question_id, index)) {
            answers.remove(&(question_id, index));
        }
        cx.notify();
    }

    pub fn can_start_mission(&self, clan_id: ClanId, index: usize) -> bool {
        self.is_finished(clan_id) || self.mission_done(clan_id) == index
    }

    pub fn complete_mission(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if !self.advance_mission(clan_id) {
            return;
        }
        cx.notify();
        let total = self
            .clans
            .get(&clan_id)
            .map_or(0, |onboarding| onboarding.missions.len());
        if total > 0 && self.mission_done.get(&clan_id).copied().unwrap_or(0) >= total {
            self.finish_onboarding(clan_id, cx);
        }
    }

    fn advance_mission(&mut self, clan_id: ClanId) -> bool {
        if self.is_finished(clan_id) {
            return false;
        }
        let total = self
            .clans
            .get(&clan_id)
            .map_or(0, |onboarding| onboarding.missions.len());
        let done = self.mission_done.entry(clan_id).or_insert(0);
        if *done >= total {
            return false;
        }
        *done += 1;
        true
    }

    fn finish_onboarding(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.steps.insert(clan_id, DONE_ONBOARDING_STATUS);
        let api = self.api.clone();
        let reset_generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            if let Err(error) = api
                .update_onboarding_step(clan_id.get(), DONE_ONBOARDING_STATUS)
                .await
            {
                tracing::warn!(%error, %clan_id, "failed to mark onboarding done");
                let _ = this.update(cx, |this, cx| {
                    if this.reset_generation != reset_generation {
                        return;
                    }
                    this.steps.remove(&clan_id);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn set_items(
        &mut self,
        clan_id: ClanId,
        items: Vec<OnboardingItem>,
        cx: &mut Context<Self>,
    ) {
        self.clans
            .insert(clan_id, ClanOnboarding::from_items(items));
        self.fetched_at.insert(clan_id, Instant::now());
        self.failed_at.remove(&clan_id);
        cx.notify();
    }

    pub fn reload(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.refetch(clan_id, cx);
        cx.notify();
    }

    fn refetch(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.fetched_at.remove(&clan_id);
        self.failed_at.remove(&clan_id);
        self.steps_fetched_at.remove(&clan_id);
        self.steps_failed_at.remove(&clan_id);
        self.ensure_loaded(clan_id, cx);
    }

    pub fn ensure_loaded(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.ensure_items_loaded(clan_id, cx);
        self.ensure_steps_loaded(clan_id, cx);
    }

    fn ensure_items_loaded(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self.loading.contains(&clan_id)
            || self
                .fetched_at
                .get(&clan_id)
                .is_some_and(|instant| now.duration_since(*instant) < CACHE_TTL)
            || self
                .failed_at
                .get(&clan_id)
                .is_some_and(|instant| now.duration_since(*instant) < FETCH_RETRY_BACKOFF)
        {
            return;
        }
        self.loading.insert(clan_id);
        let api = self.api.clone();
        let reset_generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api
                .list_onboarding(clan_id.get(), ONBOARDING_PAGE_LIMIT, 1)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != reset_generation {
                    return;
                }
                this.loading.remove(&clan_id);
                match result {
                    Ok(response) => {
                        let items = response
                            .list_onboarding
                            .into_iter()
                            .map(OnboardingItem::from)
                            .collect();
                        this.clans
                            .insert(clan_id, ClanOnboarding::from_items(items));
                        this.fetched_at.insert(clan_id, Instant::now());
                        this.failed_at.remove(&clan_id);
                    }
                    Err(error) => {
                        this.failed_at.insert(clan_id, Instant::now());
                        tracing::warn!(%error, %clan_id, "failed to load clan onboarding");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_steps_loaded(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self.steps_loading.contains(&clan_id)
            || self
                .steps_fetched_at
                .get(&clan_id)
                .is_some_and(|instant| now.duration_since(*instant) < CACHE_TTL)
            || self
                .steps_failed_at
                .get(&clan_id)
                .is_some_and(|instant| now.duration_since(*instant) < FETCH_RETRY_BACKOFF)
        {
            return;
        }
        self.steps_loading.insert(clan_id);
        let api = self.api.clone();
        let reset_generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.list_onboarding_step(clan_id.get()).await;
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != reset_generation {
                    return;
                }
                this.steps_loading.remove(&clan_id);
                match result {
                    Ok(response) => {
                        let now = Instant::now();
                        this.steps_fetched_at.insert(clan_id, now);
                        this.steps_failed_at.remove(&clan_id);
                        for step in response.list_onboarding_step {
                            let step_clan = ClanId(step.clan_id);
                            this.steps.insert(step_clan, step.onboarding_step);
                            this.steps_fetched_at.insert(step_clan, now);
                        }
                    }
                    Err(error) => {
                        this.steps_failed_at.insert(clan_id, Instant::now());
                        tracing::warn!(%error, %clan_id, "failed to load onboarding steps");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clan::OnboardingAnswer;

    fn item(id: i64, guide_type: i32, answers: usize) -> OnboardingItem {
        OnboardingItem {
            id,
            guide_type,
            answers: (0..answers)
                .map(|index| OnboardingAnswer {
                    title: format!("answer {index}"),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn test_store() -> OnboardingStore {
        OnboardingStore {
            clans: HashMap::new(),
            loading: HashSet::new(),
            fetched_at: HashMap::new(),
            failed_at: HashMap::new(),
            steps: HashMap::new(),
            steps_loading: HashSet::new(),
            steps_fetched_at: HashMap::new(),
            steps_failed_at: HashMap::new(),
            mission_done: HashMap::new(),
            answers: HashMap::new(),
            reset_generation: 0,
            api: Arc::new(AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            )),
            _connection_watch: Task::ready(()),
        }
    }

    const CLAN: ClanId = ClanId(7);

    #[test]
    fn groups_items_by_guide_type() {
        let onboarding = ClanOnboarding::from_items(vec![
            item(1, GUIDE_TYPE_GREETING, 0),
            item(2, GUIDE_TYPE_RULE, 0),
            item(3, GUIDE_TYPE_QUESTION, 2),
            item(4, GUIDE_TYPE_TASK, 0),
            item(5, GUIDE_TYPE_QUESTION, 3),
            item(6, 99, 0),
        ]);
        assert_eq!(onboarding.greeting.as_ref().map(|item| item.id), Some(1));
        assert_eq!(onboarding.rules.len(), 1);
        assert_eq!(onboarding.questions.len(), 2);
        assert_eq!(onboarding.missions.len(), 1);
        assert_eq!(onboarding.answer_count(), 5);
    }

    #[test]
    fn missions_advance_one_at_a_time_and_stop_at_total() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![
                item(1, GUIDE_TYPE_TASK, 0),
                item(2, GUIDE_TYPE_TASK, 0),
            ]),
        );
        assert!(store.can_start_mission(CLAN, 0));
        assert!(!store.can_start_mission(CLAN, 1));
        assert!(store.advance_mission(CLAN));
        assert_eq!(store.mission_done(CLAN), 1);
        assert!(store.can_start_mission(CLAN, 1));
        assert!(store.advance_mission(CLAN));
        assert!(!store.advance_mission(CLAN));
        assert_eq!(store.mission_done(CLAN), 2);
    }

    #[test]
    fn finished_onboarding_ticks_every_mission() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![
                item(1, GUIDE_TYPE_TASK, 0),
                item(2, GUIDE_TYPE_TASK, 0),
            ]),
        );
        store.steps.insert(CLAN, DONE_ONBOARDING_STATUS);
        assert_eq!(store.mission_done(CLAN), 2);
        assert!(store.can_start_mission(CLAN, 1));
        assert!(!store.advance_mission(CLAN));
    }

    #[test]
    fn load_failure_only_reports_while_no_data_is_cached() {
        let mut store = test_store();
        assert!(!store.load_failed(CLAN));
        store.failed_at.insert(CLAN, Instant::now());
        assert!(store.load_failed(CLAN));
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![item(1, GUIDE_TYPE_RULE, 0)]),
        );
        assert!(!store.load_failed(CLAN));
    }

    #[test]
    fn answer_percent_tracks_selected_answers() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![item(1, GUIDE_TYPE_QUESTION, 4)]),
        );
        assert_eq!(store.answered_percent(CLAN), 0.);
        store.answers.entry(CLAN).or_default().insert((1, 0));
        store.answers.entry(CLAN).or_default().insert((1, 2));
        assert_eq!(store.answered_percent(CLAN), 50.);
        assert!(store.answer_selected(CLAN, 1, 2));
        assert!(!store.answer_selected(CLAN, 1, 3));
    }
}
