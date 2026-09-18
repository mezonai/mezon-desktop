use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, Global, Subscription, Task};
use mezon_client::{AppApi, ConnectionStatus};

use crate::clan::{ClanEvent, ClanList, OnboardingItem};
use crate::ids::{ChannelId, ClanId};

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

#[derive(Debug, Default)]
struct PreviewRun {
    done: usize,
    answers: HashSet<(i64, usize)>,
}

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
    step_before_finish: HashMap<ClanId, i32>,
    answers: HashMap<ClanId, HashSet<(i64, usize)>>,
    preview: Option<(ClanId, PreviewRun)>,
    reset_generation: u64,
    api: Arc<AppApi>,
    _connection_watch: Task<()>,
    _clan_watch: Option<Subscription>,
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
        let clan_watch = ClanList::try_global(cx).map(|clans| {
            cx.subscribe(&clans, |this, _, event: &ClanEvent, cx| {
                if let ClanEvent::ActiveClanChanged(clan_id) = event
                    && this
                        .preview_clan()
                        .is_some_and(|previewing| clan_id.is_none_or(|active| active != previewing))
                {
                    this.close_preview(cx);
                }
            })
        });
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
            step_before_finish: HashMap::new(),
            answers: HashMap::new(),
            preview: None,
            reset_generation: 0,
            api,
            _connection_watch: connection_watch,
            _clan_watch: clan_watch,
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
        self.step_before_finish.clear();
        self.answers.clear();
        self.preview = None;
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

    pub fn load_attempted(&self, clan_id: ClanId) -> bool {
        self.loading.contains(&clan_id)
            || self.fetched_at.contains_key(&clan_id)
            || self.failed_at.contains_key(&clan_id)
    }

    pub fn is_finished(&self, clan_id: ClanId) -> bool {
        self.steps.get(&clan_id).copied() == Some(DONE_ONBOARDING_STATUS)
    }

    pub fn mission_total(&self, clan_id: ClanId) -> usize {
        self.clans
            .get(&clan_id)
            .map_or(0, |onboarding| onboarding.missions.len())
    }

    pub fn mission_progress(&self, clan_id: ClanId) -> usize {
        if let Some(run) = self.preview_run(clan_id) {
            return run.done;
        }
        if self.is_finished(clan_id) {
            return self.mission_total(clan_id);
        }
        self.mission_done.get(&clan_id).copied().unwrap_or(0)
    }

    fn preview_run(&self, clan_id: ClanId) -> Option<&PreviewRun> {
        self.preview
            .as_ref()
            .filter(|(previewing, _)| *previewing == clan_id)
            .map(|(_, run)| run)
    }

    fn preview_run_mut(&mut self, clan_id: ClanId) -> Option<&mut PreviewRun> {
        self.preview
            .as_mut()
            .filter(|(previewing, _)| *previewing == clan_id)
            .map(|(_, run)| run)
    }

    /// The mission the member is expected to do next, or `None` once every one is ticked.
    pub fn current_mission(&self, clan_id: ClanId) -> Option<&OnboardingItem> {
        self.clans
            .get(&clan_id)?
            .missions
            .get(self.mission_progress(clan_id))
    }

    /// Whether `ListOnboardingStep` has answered for this clan. The progress chrome stays hidden
    /// until it has, so a member who already finished never sees it flash on the way in.
    pub fn steps_loaded(&self, clan_id: ClanId) -> bool {
        self.steps_fetched_at.contains_key(&clan_id)
    }

    pub fn preview_clan(&self) -> Option<ClanId> {
        self.preview.as_ref().map(|(clan_id, _)| *clan_id)
    }

    pub fn is_previewing(&self, clan_id: ClanId) -> bool {
        self.preview_clan() == Some(clan_id)
    }

    /// Look at the clan the way a brand new member would: the progress chrome shows even for the
    /// owner, who has long since finished onboarding.
    pub fn open_preview(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.preview = Some((clan_id, PreviewRun::default()));
        cx.notify();
    }

    pub fn close_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview.take().is_some() {
            cx.notify();
        }
    }

    /// The gate the onboarding progress chrome shares — the sidebar card and the composer
    /// mission banner. `clan_enabled` is the clan's `is_onboarding` flag, which lives on
    /// `ClanList`.
    pub fn show_progress(&self, clan_id: ClanId, clan_enabled: bool) -> bool {
        if !clan_enabled || self.mission_total(clan_id) == 0 {
            return false;
        }
        if self.is_previewing(clan_id) {
            return true;
        }
        self.steps_loaded(clan_id) && !self.is_finished(clan_id)
    }

    pub fn answer_selected(&self, clan_id: ClanId, question_id: i64, index: usize) -> bool {
        self.selected_answers(clan_id)
            .is_some_and(|answers| answers.contains(&(question_id, index)))
    }

    pub fn answered_count(&self, clan_id: ClanId) -> usize {
        self.selected_answers(clan_id).map_or(0, HashSet::len)
    }

    fn selected_answers(&self, clan_id: ClanId) -> Option<&HashSet<(i64, usize)>> {
        match self.preview_run(clan_id) {
            Some(run) => Some(&run.answers),
            None => self.answers.get(&clan_id),
        }
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
        let answers = match self.preview_run_mut(clan_id) {
            Some(run) => &mut run.answers,
            None => self.answers.entry(clan_id).or_default(),
        };
        if !answers.insert((question_id, index)) {
            answers.remove(&(question_id, index));
        }
        cx.notify();
    }

    pub fn can_start_mission(&self, clan_id: ClanId, index: usize) -> bool {
        if !self.is_previewing(clan_id) && self.is_finished(clan_id) {
            return true;
        }
        self.mission_progress(clan_id) == index
    }

    pub fn note_message_sent(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        if self.mission_is_satisfied_by_send(clan_id, channel_id) {
            self.complete_mission(clan_id, cx);
        }
    }

    fn mission_is_satisfied_by_send(&self, clan_id: ClanId, channel_id: ChannelId) -> bool {
        self.current_mission(clan_id).is_some_and(|mission| {
            mission.task_type == MISSION_SEND_MESSAGE && mission.channel_id == channel_id.get()
        })
    }

    pub fn complete_mission(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if !self.advance_mission(clan_id) {
            return;
        }
        cx.notify();
        if self.should_persist_completion(clan_id) {
            self.finish_onboarding(clan_id, cx);
        }
    }

    /// Whether ticking the last mission should tell the server the run is over. Previewing walks
    /// the same missions to show the owner what a new member sees, so it must not record anything.
    fn should_persist_completion(&self, clan_id: ClanId) -> bool {
        if self.is_previewing(clan_id) {
            return false;
        }
        let total = self.mission_total(clan_id);
        total > 0 && self.mission_done.get(&clan_id).copied().unwrap_or(0) >= total
    }

    fn advance_mission(&mut self, clan_id: ClanId) -> bool {
        let total = self.mission_total(clan_id);
        if let Some(run) = self.preview_run_mut(clan_id) {
            if run.done >= total {
                return false;
            }
            run.done += 1;
            return true;
        }
        if self.is_finished(clan_id) {
            return false;
        }
        let done = self.mission_done.entry(clan_id).or_insert(0);
        if *done >= total {
            return false;
        }
        *done += 1;
        true
    }

    fn finish_onboarding(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let step_before = self.steps.insert(clan_id, DONE_ONBOARDING_STATUS);
        self.step_before_finish
            .insert(clan_id, step_before.unwrap_or(0));
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
                    this.revert_finish(clan_id);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Undo an optimistic finish the server refused. The tick that completed the run goes back
    /// too, so the member is left on the mission they were on instead of on a finished count the
    /// server never recorded.
    fn revert_finish(&mut self, clan_id: ClanId) {
        let step_before = self.step_before_finish.remove(&clan_id).unwrap_or(0);
        self.steps.insert(clan_id, step_before);
        if let Some(done) = self.mission_done.get_mut(&clan_id) {
            *done = done.saturating_sub(1);
        }
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
            step_before_finish: HashMap::new(),
            answers: HashMap::new(),
            preview: None,
            reset_generation: 0,
            api: Arc::new(AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            )),
            _connection_watch: Task::ready(()),
            _clan_watch: None,
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
        assert_eq!(store.mission_progress(CLAN), 1);
        assert!(store.can_start_mission(CLAN, 1));
        assert!(store.advance_mission(CLAN));
        assert!(!store.advance_mission(CLAN));
        assert_eq!(store.mission_progress(CLAN), 2);
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
        assert_eq!(store.mission_progress(CLAN), 2);
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
    fn progress_chrome_waits_for_the_step_fetch_and_hides_once_finished() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![item(1, GUIDE_TYPE_TASK, 0)]),
        );
        // Items are in, but `ListOnboardingStep` has not answered yet.
        assert!(!store.show_progress(CLAN, true));

        store.steps_fetched_at.insert(CLAN, Instant::now());
        assert!(store.show_progress(CLAN, true));

        // A clan that never switched onboarding on shows nothing either way.
        assert!(!store.show_progress(CLAN, false));
        store.steps.insert(CLAN, DONE_ONBOARDING_STATUS);
        assert!(!store.show_progress(CLAN, false));
        assert!(!store.show_progress(CLAN, true));
    }

    #[test]
    fn a_clan_with_no_missions_shows_no_progress_even_while_previewing() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![item(1, GUIDE_TYPE_RULE, 0)]),
        );
        store.steps_fetched_at.insert(CLAN, Instant::now());
        store.preview = Some((CLAN, PreviewRun::default()));

        assert!(
            !store.show_progress(CLAN, true),
            "a guide with no missions has no run to show"
        );
        assert!(!store.show_progress(CLAN, false));
    }

    #[test]
    fn preview_forces_the_chrome_on_and_restarts_the_count() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![
                item(1, GUIDE_TYPE_TASK, 0),
                item(2, GUIDE_TYPE_TASK, 0),
            ]),
        );
        store.steps.insert(CLAN, DONE_ONBOARDING_STATUS);
        assert!(!store.show_progress(CLAN, true));
        assert_eq!(store.mission_progress(CLAN), 2);

        store.preview = Some((CLAN, PreviewRun::default()));
        assert!(store.show_progress(CLAN, true));
        // The finished flag ticks every mission on the guide, but the owner previewing the clan
        // still starts the run from the first one.
        assert_eq!(store.mission_progress(CLAN), 0);
        assert_eq!(store.current_mission(CLAN).map(|item| item.id), Some(1));
        assert!(!store.show_progress(ClanId(8), true));
    }

    #[test]
    fn preview_walks_the_missions_an_owner_already_finished() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![
                item(1, GUIDE_TYPE_TASK, 0),
                item(2, GUIDE_TYPE_TASK, 0),
            ]),
        );
        store.steps.insert(CLAN, DONE_ONBOARDING_STATUS);
        store.preview = Some((CLAN, PreviewRun::default()));

        assert!(store.can_start_mission(CLAN, 0));
        assert!(
            !store.can_start_mission(CLAN, 1),
            "previewing follows the run in order, like a new member"
        );
        assert!(store.advance_mission(CLAN));
        assert_eq!(store.mission_progress(CLAN), 1);
        assert_eq!(store.current_mission(CLAN).map(|item| item.id), Some(2));
        assert!(store.advance_mission(CLAN));
        assert_eq!(store.mission_progress(CLAN), 2);
        assert!(store.current_mission(CLAN).is_none());
    }

    #[test]
    fn previewing_never_records_the_run_as_finished() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![item(1, GUIDE_TYPE_TASK, 0)]),
        );
        store.mission_done.insert(CLAN, 1);
        assert!(store.should_persist_completion(CLAN));

        store.preview = Some((CLAN, PreviewRun::default()));
        assert!(!store.should_persist_completion(CLAN));
    }

    #[test]
    fn a_step_the_server_refused_takes_the_last_tick_back_with_it() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![
                item(1, GUIDE_TYPE_TASK, 0),
                item(2, GUIDE_TYPE_TASK, 0),
            ]),
        );
        store.mission_done.insert(CLAN, 2);
        store.steps.insert(CLAN, DONE_ONBOARDING_STATUS);

        store.revert_finish(CLAN);

        assert!(!store.is_finished(CLAN));
        assert_eq!(
            store.mission_progress(CLAN),
            1,
            "the member is left on the mission they were on, not on a finished count"
        );
        assert_eq!(store.current_mission(CLAN).map(|item| item.id), Some(2));
    }

    #[test]
    fn no_current_mission_once_every_one_is_done() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![item(1, GUIDE_TYPE_TASK, 0)]),
        );
        assert_eq!(store.current_mission(CLAN).map(|item| item.id), Some(1));
        store.mission_done.insert(CLAN, 1);
        assert!(store.current_mission(CLAN).is_none());
    }

    #[test]
    fn previewing_leaves_the_members_own_run_untouched() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![
                item(1, GUIDE_TYPE_TASK, 0),
                item(2, GUIDE_TYPE_TASK, 0),
            ]),
        );
        store.steps.insert(CLAN, 0);
        store.mission_done.insert(CLAN, 1);

        store.preview = Some((CLAN, PreviewRun::default()));
        assert_eq!(
            store.mission_progress(CLAN),
            0,
            "preview starts its own run"
        );
        assert!(store.advance_mission(CLAN));
        assert!(store.advance_mission(CLAN));
        assert_eq!(store.mission_progress(CLAN), 2);
        assert!(!store.should_persist_completion(CLAN));

        store.preview = None;
        assert_eq!(
            store.mission_progress(CLAN),
            1,
            "the member is still on the mission they were really on"
        );
        assert!(
            store.advance_mission(CLAN),
            "their own run must still be completable"
        );
        assert!(store.should_persist_completion(CLAN));
    }

    #[test]
    fn every_preview_starts_over() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![
                item(1, GUIDE_TYPE_TASK, 0),
                item(2, GUIDE_TYPE_TASK, 0),
            ]),
        );
        store.preview = Some((CLAN, PreviewRun::default()));
        assert!(store.advance_mission(CLAN));
        assert_eq!(store.mission_progress(CLAN), 1);

        store.preview = Some((CLAN, PreviewRun::default()));
        assert_eq!(store.mission_progress(CLAN), 0);
        assert_eq!(store.current_mission(CLAN).map(|item| item.id), Some(1));
    }

    #[test]
    fn preview_answers_do_not_leak_into_the_real_ones() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![item(1, GUIDE_TYPE_QUESTION, 4)]),
        );
        store.answers.entry(CLAN).or_default().insert((1, 0));

        store.preview = Some((CLAN, PreviewRun::default()));
        assert!(
            !store.answer_selected(CLAN, 1, 0),
            "preview starts unanswered"
        );
        store
            .preview_run_mut(CLAN)
            .expect("previewing")
            .answers
            .insert((1, 3));
        assert_eq!(store.answered_count(CLAN), 1);

        store.preview = None;
        assert!(store.answer_selected(CLAN, 1, 0));
        assert!(!store.answer_selected(CLAN, 1, 3));
        assert_eq!(store.answered_count(CLAN), 1);
    }

    #[test]
    fn a_refused_step_keeps_the_chrome_on_screen() {
        let mut store = test_store();
        store.clans.insert(
            CLAN,
            ClanOnboarding::from_items(vec![
                item(1, GUIDE_TYPE_TASK, 0),
                item(2, GUIDE_TYPE_TASK, 0),
            ]),
        );
        store.steps_fetched_at.insert(CLAN, Instant::now());
        store.steps.insert(CLAN, 0);
        store.mission_done.insert(CLAN, 2);

        store.step_before_finish.insert(CLAN, 0);
        store.steps.insert(CLAN, DONE_ONBOARDING_STATUS);
        store.revert_finish(CLAN);

        assert!(!store.is_finished(CLAN));
        assert!(
            store.show_progress(CLAN, true),
            "a refused step must not take the card and the banner with it"
        );
        assert_eq!(store.mission_progress(CLAN), 1);
        assert_eq!(store.current_mission(CLAN).map(|item| item.id), Some(2));
    }

    #[test]
    fn a_send_message_mission_is_ticked_by_the_send_that_satisfies_it() {
        let mut store = test_store();
        let mut mission = item(1, GUIDE_TYPE_TASK, 0);
        mission.task_type = MISSION_SEND_MESSAGE;
        mission.channel_id = 42;
        let mut second = item(2, GUIDE_TYPE_TASK, 0);
        second.task_type = MISSION_VISIT;
        store
            .clans
            .insert(CLAN, ClanOnboarding::from_items(vec![mission, second]));

        assert_eq!(store.current_mission(CLAN).map(|item| item.id), Some(1));
        assert!(
            !store.mission_is_satisfied_by_send(CLAN, ChannelId(7)),
            "a send elsewhere is not this mission"
        );
        assert!(store.mission_is_satisfied_by_send(CLAN, ChannelId(42)));
    }

    #[test]
    fn a_load_that_was_attempted_stops_the_sidebar_asking_again() {
        let mut store = test_store();
        assert!(!store.load_attempted(CLAN));
        store.failed_at.insert(CLAN, Instant::now());
        assert!(store.load_attempted(CLAN));
        store.failed_at.remove(&CLAN);
        store.fetched_at.insert(CLAN, Instant::now());
        assert!(store.load_attempted(CLAN));
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
