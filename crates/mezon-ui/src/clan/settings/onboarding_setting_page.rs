use std::path::PathBuf;

use gpui::{
    App, Context, Entity, Focusable, FontWeight, InteractiveElement, ListAlignment, ListState,
    PathPromptOptions, SharedString, StatefulInteractiveElement, Task, Window, deferred, div, img,
    linear_color_stop, linear_gradient, list, prelude::*, px, rgb,
};
use mezon_store::{
    ChannelList, ClanId, ClanList, ClanMembersStore, OnboardingAnswer, OnboardingContent,
    OnboardingItem, Settings,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Input, InputState, Sizable, Size,
    UnsavedChangesBar, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};
use crate::util::theme::theme_is_light;

use super::onboarding_modal;

const GUIDE_RULE: i32 = 2;
const GUIDE_TASK: i32 = 3;
const GUIDE_QUESTION: i32 = 4;
const TASK_SEND_MESSAGE: i32 = 1;
const TASK_VISIT: i32 = 2;
const TASK_READ: i32 = 3;
const MAX_ANSWERS: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Main,
    Questions,
    Guide,
}

#[derive(Clone)]
struct AnswerDraft {
    title: String,
    description: String,
}

#[derive(Clone)]
struct QuestionDraft {
    id: Option<i64>,
    title: String,
    answers: Vec<AnswerDraft>,
    expanded: bool,
    title_input: Option<Entity<InputState>>,
}

#[derive(Clone)]
struct MissionDraft {
    id: Option<i64>,
    title: String,
    channel_id: i64,
    task_type: i32,
}

#[derive(Clone)]
struct ResourceDraft {
    id: Option<i64>,
    title: String,
    description: String,
    image_url: String,
    image_path: Option<PathBuf>,
}

enum Editor {
    Answer {
        question: usize,
        answer: Option<usize>,
        title: Entity<InputState>,
        description: Entity<InputState>,
    },
    Mission {
        index: Option<usize>,
        title: Entity<InputState>,
        channel_id: i64,
        task_type: i32,
        channels_open: bool,
    },
    Resource {
        index: Option<usize>,
        title: Entity<InputState>,
        description: Entity<InputState>,
        image_url: String,
        image_path: Option<PathBuf>,
    },
}

pub struct OnboardingSettingPage {
    clan_id: ClanId,
    clan_list: Entity<ClanList>,
    channel_list: Entity<ChannelList>,
    settings: Entity<Settings>,
    enabled: bool,
    loading: bool,
    load_error: Option<String>,
    saving: bool,
    setup_open: bool,
    page: Page,
    questions: Vec<QuestionDraft>,
    question_list: ListState,
    missions: Vec<MissionDraft>,
    mission_list: ListState,
    resources: Vec<ResourceDraft>,
    resource_list: ListState,
    original_ids: Vec<i64>,
    dirty: bool,
    channels_expanded: bool,
    editor: Option<Editor>,
    editor_generation: u64,
    _fetch_task: Option<Task<()>>,
    _mutation_task: Option<Task<()>>,
    _upload_task: Option<Task<()>>,
}

impl OnboardingSettingPage {
    pub fn is_setup_open(&self) -> bool {
        self.setup_open
    }

    pub fn is_editor_open(&self) -> bool {
        self.editor.is_some()
    }

    pub fn render_enable_card(
        page: Entity<Self>,
        locale: &str,
        _theme: &Theme,
    ) -> gpui::AnyElement {
        let enable = page.downgrade();
        v_flex()
            .relative()
            .overflow_hidden()
            .items_center()
            .min_h(px(718.0))
            .rounded(px(16.0))
            .bg(rgb(0x4752c4))
            .text_color(gpui::white())
            .shadow_lg()
            .child(
                div()
                    .absolute()
                    .top(px(-64.0))
                    .left(px(-64.0))
                    .size(px(128.0))
                    .rounded_full()
                    .bg(gpui::white().alpha(0.05)),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(-96.0))
                    .right(px(-96.0))
                    .size(px(192.0))
                    .rounded_full()
                    .bg(gpui::white().alpha(0.05)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(360.0))
                    .left(px(-48.0))
                    .size(px(96.0))
                    .rounded_full()
                    .bg(gpui::white().alpha(0.1)),
            )
            .child(
                v_flex()
                    .relative()
                    .items_center()
                    .w_full()
                    .px_8()
                    .pt(px(36.0))
                    .child(
                        div()
                            .mb_8()
                            .p_4()
                            .rounded(px(16.0))
                            .bg(gpui::white().alpha(0.1))
                            .shadow_lg()
                            .child(
                                img(crate::util::assets::ONBOARDING)
                                    .w(px(350.0))
                                    .rounded(px(8.0)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(36.0))
                            .font_weight(FontWeight::BOLD)
                            .text_center()
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.enableOnboarding.title",
                            )),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .h(px(4.0))
                            .mt_3()
                            .mb_6()
                            .rounded_full()
                            .bg(gpui::white().alpha(0.3)),
                    )
                    .child(
                        div()
                            .text_lg()
                            .text_center()
                            .text_color(gpui::white().alpha(0.9))
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.enableOnboarding.description",
                            )),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_base()
                            .text_center()
                            .text_color(gpui::white().alpha(0.8))
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.enableOnboarding.subtitle",
                            )),
                    )
                    .child(
                        div()
                            .id("enable-onboarding")
                            .mt_8()
                            .px(px(40.0))
                            .py(px(16.0))
                            .rounded(px(12.0))
                            .bg(gpui::white())
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0x5865f2))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0xf9fafb)))
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.enableOnboarding.button",
                            ))
                            .on_click(move |_, _, cx| {
                                let _ = enable.update(cx, |this, cx| {
                                    this.setup_open = true;
                                    this.page = Page::Main;
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        div()
                            .mt_6()
                            .text_sm()
                            .text_color(gpui::white().alpha(0.6))
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.enableOnboarding.note",
                            )),
                    ),
            )
            .into_any_element()
    }

    pub fn new(
        clan_id: ClanId,
        clan_list: Entity<ClanList>,
        channel_list: Entity<ChannelList>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let enabled = clan_list
            .read(cx)
            .clan_by_id(clan_id)
            .is_some_and(|clan| clan.is_onboarding);
        let mut this = Self {
            clan_id,
            clan_list,
            channel_list,
            settings,
            enabled,
            loading: true,
            load_error: None,
            saving: false,
            setup_open: false,
            page: Page::Main,
            questions: Vec::new(),
            question_list: ListState::new(0, ListAlignment::Top, px(120.0))
                .suppress_hover_while_scrolling(),
            missions: Vec::new(),
            mission_list: ListState::new(0, ListAlignment::Top, px(100.0))
                .suppress_hover_while_scrolling(),
            resources: Vec::new(),
            resource_list: ListState::new(0, ListAlignment::Top, px(100.0))
                .suppress_hover_while_scrolling(),
            original_ids: Vec::new(),
            dirty: false,
            channels_expanded: false,
            editor: None,
            editor_generation: 0,
            _fetch_task: None,
            _mutation_task: None,
            _upload_task: None,
        };
        this.fetch(cx);
        this
    }

    pub fn release(&mut self) {
        self._fetch_task.take();
        self._mutation_task.take();
        self._upload_task.take();
        self.editor_generation = self.editor_generation.wrapping_add(1);
        self.editor = None;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn should_show_save_bar(&self) -> bool {
        self.enabled && self.dirty && !self.setup_open && self.editor.is_none() && !self.saving
    }

    fn locale(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn list_can_consume_scroll(state: &ListState, delta_y: gpui::Pixels) -> bool {
        if delta_y < px(0.0) {
            matches!(state.is_scrolled_to_end(), Some(false))
        } else if delta_y > px(0.0) {
            let offset = state.logical_scroll_top();
            offset.item_ix > 0 || offset.offset_in_item > px(0.0)
        } else {
            false
        }
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.load_error = None;
        self.editor_generation = self.editor_generation.wrapping_add(1);
        self.editor = None;
        let clan_id = self.clan_id;
        let task = self
            .clan_list
            .update(cx, |store, cx| store.fetch_onboarding(clan_id, cx));
        self._fetch_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(items) => this.apply_items(items),
                    Err(error) => {
                        tracing::error!("fetch onboarding failed: {error}");
                        this.load_error = Some(error);
                    }
                }
                cx.notify();
            });
        }));
    }

    fn apply_items(&mut self, items: Vec<OnboardingItem>) {
        self.original_ids = items
            .iter()
            .filter(|item| matches!(item.guide_type, GUIDE_RULE | GUIDE_TASK | GUIDE_QUESTION))
            .map(|item| item.id)
            .collect();
        self.questions.clear();
        self.missions.clear();
        self.resources.clear();
        for item in items {
            match item.guide_type {
                GUIDE_QUESTION => self.questions.push(QuestionDraft {
                    id: Some(item.id),
                    title: item.title,
                    answers: item
                        .answers
                        .into_iter()
                        .map(|answer| AnswerDraft {
                            title: answer.title,
                            description: answer.description,
                        })
                        .collect(),
                    expanded: false,
                    title_input: None,
                }),
                GUIDE_TASK => self.missions.push(MissionDraft {
                    id: Some(item.id),
                    title: item.title,
                    channel_id: item.channel_id,
                    task_type: item.task_type,
                }),
                GUIDE_RULE => self.resources.push(ResourceDraft {
                    id: Some(item.id),
                    title: item.title,
                    description: item.content,
                    image_url: item.image_url,
                    image_path: None,
                }),
                _ => {}
            }
        }
        self.question_list.reset(self.questions.len());
        self.mission_list.reset(self.missions.len());
        self.resource_list.reset(self.resources.len());
        self.dirty = false;
    }

    fn ensure_question_input(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.locale(cx);
        if let Some(question) = self.questions.get_mut(index)
            && question.title_input.is_none()
        {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(mezon_i18n::t(
                        &locale,
                        "onBoardingClan.questionsPage.enterQuestion",
                    ))
                    .height(px(46.0))
            });
            input.update(cx, |state, cx| {
                state.set_value(question.title.clone(), window, cx)
            });
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            question.title_input = Some(input);
        }
    }

    fn sync_question_titles(&mut self, cx: &App) {
        for question in &mut self.questions {
            if let Some(input) = &question.title_input {
                question.title = input.read(cx).value().trim().to_string();
            }
        }
    }

    fn add_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.locale(cx);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(mezon_i18n::t(
                    &locale,
                    "onBoardingClan.questionsPage.enterQuestion",
                ))
                .height(px(46.0))
        });
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        let question_index = self.questions.len();
        self.questions.push(QuestionDraft {
            id: None,
            title: String::new(),
            answers: Vec::new(),
            expanded: true,
            title_input: Some(input),
        });
        self.question_list.splice(question_index..question_index, 1);
        self.question_list.scroll_to_end();
        self.dirty = true;
        cx.notify();
    }

    fn open_answer_editor(
        &mut self,
        question: usize,
        answer: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(question_draft) = self.questions.get(question) else {
            return;
        };
        let locale = self.locale(cx);
        let existing = answer.and_then(|index| question_draft.answers.get(index));
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(mezon_i18n::t(
                    &locale,
                    "onBoardingClan.questionsPage.enterAnswer",
                ))
                .height(px(48.0))
        });
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(mezon_i18n::t(
                    &locale,
                    "onBoardingClan.questionsPage.enterDescription",
                ))
                .height(px(48.0))
        });
        if let Some(existing) = existing {
            title.update(cx, |state, cx| {
                state.set_value(existing.title.clone(), window, cx)
            });
            description.update(cx, |state, cx| {
                state.set_value(existing.description.clone(), window, cx)
            });
        }
        cx.observe(&title, |_, _, cx| cx.notify()).detach();
        cx.observe(&description, |_, _, cx| cx.notify()).detach();
        self.editor_generation = self.editor_generation.wrapping_add(1);
        self.editor = Some(Editor::Answer {
            question,
            answer,
            title: title.clone(),
            description,
        });
        let focus = title.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn open_mission_editor(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = index.and_then(|index| self.missions.get(index)).cloned();
        let locale = self.locale(cx);
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(mezon_i18n::t(
                    &locale,
                    "onboardingMissions.form.placeholder",
                ))
                .height(px(48.0))
        });
        if let Some(existing) = &existing {
            title.update(cx, |state, cx| {
                state.set_value(existing.title.clone(), window, cx)
            });
        }
        cx.observe(&title, |_, _, cx| cx.notify()).detach();
        let default_channel = self
            .public_channels(cx)
            .first()
            .map(|(id, _)| *id)
            .unwrap_or(0);
        self.editor_generation = self.editor_generation.wrapping_add(1);
        self.editor = Some(Editor::Mission {
            index,
            title,
            channel_id: existing
                .as_ref()
                .map_or(default_channel, |item| item.channel_id),
            task_type: existing
                .as_ref()
                .map_or(TASK_SEND_MESSAGE, |item| item.task_type),
            channels_open: false,
        });
        cx.notify();
    }

    fn open_resource_editor(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let locale = self.locale(cx);
        let existing = index.and_then(|index| self.resources.get(index)).cloned();
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(mezon_i18n::t(
                    &locale,
                    "onboardingRules.form.namePlaceholder",
                ))
                .height(px(48.0))
        });
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(mezon_i18n::t(
                    &locale,
                    "onboardingRules.form.descriptionPlaceholder",
                ))
                .height(px(48.0))
        });
        if let Some(existing) = &existing {
            title.update(cx, |state, cx| {
                state.set_value(existing.title.clone(), window, cx)
            });
            description.update(cx, |state, cx| {
                state.set_value(existing.description.clone(), window, cx)
            });
        }
        cx.observe(&title, |_, _, cx| cx.notify()).detach();
        cx.observe(&description, |_, _, cx| cx.notify()).detach();
        self.editor_generation = self.editor_generation.wrapping_add(1);
        self.editor = Some(Editor::Resource {
            index,
            title,
            description,
            image_url: existing
                .as_ref()
                .map_or_else(String::new, |item| item.image_url.clone()),
            image_path: existing.and_then(|item| item.image_path),
        });
        cx.notify();
    }

    fn public_channels(&self, cx: &App) -> Vec<(i64, String)> {
        self.channel_list
            .read(cx)
            .user_channels()
            .filter(|channel| channel.clan_id == self.clan_id && !channel.private)
            .map(|channel| (channel.id.get(), channel.name.clone()))
            .collect()
    }

    fn cancel_setup(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.setup_open = false;
        self.page = Page::Main;
        self.editor_generation = self.editor_generation.wrapping_add(1);
        self.editor = None;
        self.fetch(cx);
    }

    fn reset_changes(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.editor_generation = self.editor_generation.wrapping_add(1);
        self.editor = None;
        self.fetch(cx);
    }

    fn save_all(&mut self, enable_after: bool, cx: &mut Context<Self>) {
        if self.loading || self.load_error.is_some() || self.saving {
            return;
        }
        self.sync_question_titles(cx);
        if self
            .questions
            .iter()
            .any(|question| question.title.is_empty())
        {
            Shell::global(cx).update(cx, |shell, cx| {
                shell.error(
                    mezon_i18n::t(&self.locale(cx), "onBoardingClan.errors.questionRequired")
                        .to_string(),
                    cx,
                )
            });
            return;
        }
        let mut contents = Vec::new();
        let mut existing = Vec::new();
        for question in &self.questions {
            let content = OnboardingContent {
                guide_type: GUIDE_QUESTION,
                title: question.title.clone(),
                answers: question
                    .answers
                    .iter()
                    .map(|answer| OnboardingAnswer {
                        title: answer.title.clone(),
                        description: answer.description.clone(),
                    })
                    .collect(),
                ..Default::default()
            };
            if let Some(id) = question.id {
                existing.push((id, content));
            } else {
                contents.push(content);
            }
        }
        for mission in &self.missions {
            let content = OnboardingContent {
                guide_type: GUIDE_TASK,
                task_type: mission.task_type,
                channel_id: mission.channel_id,
                title: mission.title.clone(),
                ..Default::default()
            };
            if let Some(id) = mission.id {
                existing.push((id, content));
            } else {
                contents.push(content);
            }
        }
        for resource in &self.resources {
            let content = OnboardingContent {
                guide_type: GUIDE_RULE,
                title: resource.title.clone(),
                content: resource.description.clone(),
                image_url: resource.image_url.clone(),
                ..Default::default()
            };
            if let Some(id) = resource.id {
                existing.push((id, content));
            } else {
                contents.push(content);
            }
        }
        if contents.is_empty() && existing.is_empty() {
            Shell::global(cx).update(cx, |shell, cx| {
                shell.error(
                    mezon_i18n::t(&self.locale(cx), "onBoardingClan.errors.needAtLeastOneItem")
                        .to_string(),
                    cx,
                )
            });
            return;
        }
        let kept: std::collections::HashSet<i64> = existing.iter().map(|(id, _)| *id).collect();
        let deleted: Vec<i64> = self
            .original_ids
            .iter()
            .copied()
            .filter(|id| !kept.contains(id))
            .collect();
        let clan_id = self.clan_id;
        self.saving = true;
        let create_task = (!contents.is_empty()).then(|| {
            self.clan_list.update(cx, |store, cx| {
                store.create_onboarding_items(clan_id, contents, cx)
            })
        });
        let update_tasks: Vec<_> = existing
            .into_iter()
            .map(|(id, content)| {
                self.clan_list.update(cx, |store, cx| {
                    store.update_onboarding_item(clan_id, id, content, cx)
                })
            })
            .collect();
        let delete_tasks: Vec<_> = deleted
            .into_iter()
            .map(|id| {
                self.clan_list.update(cx, |store, cx| {
                    store.delete_onboarding_item(clan_id, id, cx)
                })
            })
            .collect();
        let enable_task = enable_after.then(|| {
            self.clan_list.update(cx, |store, cx| {
                store.set_onboarding_enabled(clan_id, true, cx)
            })
        });
        let locale = self.locale(cx);
        self._mutation_task = Some(cx.spawn(async move |this, cx| {
            let mut result = Ok(());
            if let Some(task) = create_task {
                result = task.await.map(|_| ());
            }
            if result.is_ok() {
                for task in update_tasks {
                    if let Err(error) = task.await {
                        result = Err(error);
                        break;
                    }
                }
            }
            if result.is_ok() {
                for task in delete_tasks {
                    if let Err(error) = task.await {
                        result = Err(error);
                        break;
                    }
                }
            }
            if result.is_ok()
                && let Some(task) = enable_task
            {
                result = task.await;
            }
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(()) => {
                        this.enabled |= enable_after;
                        this.setup_open = false;
                        this.page = Page::Main;
                        this.dirty = false;
                        Shell::global(cx).update(cx, |shell, cx| {
                            shell.success(
                                mezon_i18n::t(
                                    &locale,
                                    "onBoardingClan.messages.changesSavedSuccess",
                                )
                                .to_string(),
                                cx,
                            )
                        });
                        this.fetch(cx);
                    }
                    Err(error) => {
                        Shell::global(cx).update(cx, |shell, cx| shell.error(error, cx));
                        this.fetch(cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn disable(&mut self, cx: &mut Context<Self>) {
        let clan_id = self.clan_id;
        self.saving = true;
        let task = self.clan_list.update(cx, |store, cx| {
            store.set_onboarding_enabled(clan_id, false, cx)
        });
        self._mutation_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(()) => this.enabled = false,
                    Err(error) => Shell::global(cx).update(cx, |shell, cx| shell.error(error, cx)),
                }
                cx.notify();
            });
        }));
    }

    fn button(
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        primary: bool,
        _theme: &Theme,
    ) -> Button {
        let label = label.into();
        Button::new(id)
            .label(label)
            .with_size(Size::Medium)
            .with_variant(if primary {
                crate::components::primitives::ButtonVariant::Primary
            } else {
                crate::components::primitives::ButtonVariant::Secondary
            })
    }

    fn render_main(&self, theme: &Theme, locale: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let setup = cx.entity().downgrade();
        let guide = cx.entity().downgrade();
        v_flex()
            .gap_5()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(mezon_i18n::t(locale, "onBoardingClan.onboarding.title")),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(div().text_color(theme.text_secondary).child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.onboarding.description",
                            )))
                            .child(
                                div()
                                    .text_color(rgb(0x5865f2))
                                    .child(mezon_i18n::t(locale, "common.preview")),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .rounded(px(8.0))
                    .bg(theme.bg_secondary)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        v_flex()
                            .p_4()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.onboarding.enabledTitle",
                            )))
                            .child(div().text_sm().text_color(theme.text_secondary).child(
                                mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.onboarding.enabledDescription",
                                ),
                            )),
                    )
                    .child(self.main_row(
                        mezon_i18n::t(locale, "onBoardingClan.questions.title").into(),
                        mezon_i18n::t(locale, "onBoardingClan.questions.description").into(),
                        IconName::People,
                        "onboarding-setup-questions",
                        mezon_i18n::t(locale, "onBoardingClan.buttons.setup").into(),
                        true,
                        theme,
                        move |_, _, cx| {
                            let _ = setup.update(cx, |this, cx| {
                                this.page = Page::Questions;
                                cx.notify();
                            });
                        },
                    ))
                    .child(self.main_row(
                        mezon_i18n::t(locale, "onBoardingClan.clanGuide.title").into(),
                        mezon_i18n::t(locale, "onBoardingClan.clanGuide.description").into(),
                        IconName::GuideIcon,
                        "onboarding-edit-guide",
                        mezon_i18n::t(locale, "onBoardingClan.buttons.edit").into(),
                        false,
                        theme,
                        move |_, _, cx| {
                            let _ = guide.update(cx, |this, cx| {
                                this.page = Page::Guide;
                                cx.notify();
                            });
                        },
                    )),
            )
            .into_any_element()
    }

    fn main_row(
        &self,
        title: SharedString,
        description: SharedString,
        icon: IconName,
        action_id: &'static str,
        action: SharedString,
        show_arrow: bool,
        theme: &Theme,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> gpui::AnyElement {
        let row_icon = if icon == IconName::GuideIcon {
            img(icon.path())
                .size(px(26.0))
                .flex_none()
                .into_any_element()
        } else {
            Icon::new(icon)
                .size(px(26.0))
                .text_color(theme.text_secondary)
                .into_any_element()
        };

        h_flex()
            .id(format!("{action_id}-row"))
            .w_full()
            .justify_between()
            .items_center()
            .p_4()
            .border_t_1()
            .border_color(theme.border)
            .hover(|style| style.bg(theme.bg_hover))
            .child(
                h_flex().gap_4().child(row_icon).child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_secondary)
                                .child(description),
                        ),
                ),
            )
            .child(
                h_flex()
                    .id(action_id)
                    .gap_2()
                    .h(px(34.0))
                    .px_3()
                    .rounded_md()
                    .bg(theme.brand)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.brand_hover))
                    .child(action)
                    .when(show_arrow, |button| {
                        button.child(
                            Icon::new(IconName::LongArrowRight)
                                .size_4()
                                .text_color(theme.text_primary),
                        )
                    })
                    .on_click(on_click),
            )
            .into_any_element()
    }

    fn render_questions(
        &mut self,
        theme: &Theme,
        locale: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let back = cx.entity().downgrade();
        let expand_channels = cx.entity().downgrade();
        let add = cx.entity().downgrade();
        if self.question_list.item_count() != self.questions.len() {
            self.question_list.reset(self.questions.len());
        }
        let entity = cx.entity();
        let list_theme = theme.clone();
        let list_locale = SharedString::from(locale.to_string());
        let list_state = self.question_list.clone();
        let scroll_state = list_state.clone();
        let question_list_height = if self.questions.iter().any(|question| question.expanded) {
            360.0
        } else {
            (self.questions.len() as f32 * 104.0).min(360.0)
        };
        let question_rows = div()
            .w_full()
            .h(px(question_list_height))
            .on_scroll_wheel(move |event, window, cx| {
                let delta_y = event.delta.pixel_delta(window.line_height()).y;
                if Self::list_can_consume_scroll(&scroll_state, delta_y) {
                    cx.stop_propagation();
                }
            })
            .child(
                list(list_state, move |index, _window, cx| {
                    entity.update(cx, |this, cx| {
                        this.render_question(index, &list_theme, &list_locale, cx)
                    })
                })
                .size_full(),
            );
        v_flex()
            .gap_5()
            .child(
                div()
                    .id("onboarding-questions-back")
                    .cursor_pointer()
                    .text_color(theme.text_secondary)
                    .child(format!(
                        "← {}",
                        mezon_i18n::t(locale, "onBoardingClan.buttons.back")
                    ))
                    .on_click(move |_, _, cx| {
                        let _ = back.update(cx, |this, cx| {
                            this.page = Page::Main;
                            cx.notify();
                        });
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(mezon_i18n::t(locale, "onBoardingClan.questionsPage.title")),
                    )
                    .child(div().text_color(theme.text_secondary).child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.questionsPage.description",
                    ))),
            )
            .child(
                v_flex()
                    .id("onboarding-unassigned-channels")
                    .rounded(px(8.0))
                    .bg(theme.bg_secondary)
                    .child(
                        h_flex()
                            .id("onboarding-unassigned-channels-trigger")
                            .p_3()
                            .justify_between()
                            .cursor_pointer()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(
                                mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.questionsPage.noChannelsMissing",
                                ),
                            ))
                            .child(
                                h_flex()
                                    .gap_4()
                                    .child(
                                        div()
                                            .w(px(150.0))
                                            .h(px(6.0))
                                            .rounded_full()
                                            .bg(theme.border)
                                            .child(
                                                div()
                                                    .w(px(105.0))
                                                    .h_full()
                                                    .rounded_full()
                                                    .bg(theme.status_online),
                                            ),
                                    )
                                    .child(
                                        Icon::new(IconName::ArrowRight)
                                            .size_4()
                                            .text_color(theme.text_secondary)
                                            .when(self.channels_expanded, |icon| {
                                                icon.with_transformation(
                                                    gpui::Transformation::rotate(gpui::radians(
                                                        std::f32::consts::FRAC_PI_2,
                                                    )),
                                                )
                                            }),
                                    ),
                            )
                            .on_click(move |_, _, cx| {
                                let _ = expand_channels.update(cx, |this, cx| {
                                    this.channels_expanded = !this.channels_expanded;
                                    cx.notify();
                                });
                            }),
                    )
                    .when(self.channels_expanded, |el| {
                        el.child(
                            v_flex()
                                .p_3()
                                .border_t_1()
                                .border_color(theme.border)
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(
                                    mezon_i18n::t(
                                        locale,
                                        "onBoardingClan.questionsPage.channelNotAssigned",
                                    ),
                                ))
                                .child(div().text_sm().text_color(theme.text_secondary).child(
                                    mezon_i18n::t(
                                        locale,
                                        "onBoardingClan.questionsPage.noChannelsHere",
                                    ),
                                )),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().font_weight(FontWeight::BOLD).child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.questionsPage.preJoinQuestions.title",
                    )))
                    .child(div().text_color(theme.text_secondary).child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.questionsPage.preJoinQuestions.description",
                    ))),
            )
            .when(!self.questions.is_empty(), |page| page.child(question_rows))
            .child(
                div()
                    .id("onboarding-add-question")
                    .p_4()
                    .rounded(px(8.0))
                    .border_2()
                    .border_dashed()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .text_center()
                    .text_color(rgb(0x949cf7))
                    .hover(|style| style.border_color(theme.text_secondary).bg(theme.bg_hover))
                    .child(
                        h_flex()
                            .justify_center()
                            .gap_2()
                            .font_weight(FontWeight::BOLD)
                            .child(
                                Icon::new(IconName::CirclePlusFill)
                                    .size_5()
                                    .text_color(rgb(0x949cf7)),
                            )
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.questionsPage.addQuestion",
                            )),
                    )
                    .on_click(move |_, window, cx| {
                        let _ = add.update(cx, |this, cx| this.add_question(window, cx));
                    }),
            )
            .into_any_element()
    }

    fn render_question(
        &self,
        index: usize,
        theme: &Theme,
        locale: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let question = &self.questions[index];
        let question_valid = question
            .title_input
            .as_ref()
            .is_some_and(|input| !input.read(cx).value().trim().is_empty());
        let remove = cx.entity().downgrade();
        let toggle = cx.entity().downgrade();
        let trash_group = SharedString::from(format!("onboarding-trash-question-{index}"));
        let save = cx.entity().downgrade();
        let add_answer = cx.entity().downgrade();
        let mut card = v_flex()
            .w_full()
            .gap_3()
            .p_4()
            .rounded(px(8.0))
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_secondary)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .child(
                        div().text_size(px(12.)).child(
                            format!(
                                "{} {}",
                                mezon_i18n::t(locale, "onBoardingClan.questionsPage.title"),
                                index + 1
                            )
                            .to_uppercase(),
                        ),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div()
                                    .id(("remove-question", index))
                                    .group(trash_group.clone())
                                    .p_1()
                                    .cursor_pointer()
                                    .text_color(theme.text_secondary)
                                    .hover(|style| style.text_color(theme.danger_text))
                                    .child(
                                        Icon::new(IconName::TrashIcon)
                                            .size_4()
                                            .text_color(theme.text_secondary)
                                            .group_hover(trash_group, |style| {
                                                style.text_color(theme.danger_text)
                                            }),
                                    )
                                    .on_click(move |_, _, cx| {
                                        let _ = remove.update(cx, |this, cx| {
                                            if index < this.questions.len() {
                                                this.questions.remove(index);
                                                this.question_list.splice(index..index + 1, 0);
                                                this.dirty = true;
                                            }
                                            cx.notify();
                                        });
                                    }),
                            )
                            .child(
                                div()
                                    .id(("toggle-question", index))
                                    .p_1()
                                    .cursor_pointer()
                                    .text_color(theme.text_secondary)
                                    .child(
                                        Icon::new(IconName::ArrowRight)
                                            .size_4()
                                            .text_color(theme.text_secondary)
                                            .when(question.expanded, |icon| {
                                                icon.with_transformation(
                                                    gpui::Transformation::rotate(gpui::radians(
                                                        std::f32::consts::FRAC_PI_2,
                                                    )),
                                                )
                                            }),
                                    )
                                    .on_click(move |_, window, cx| {
                                        let _ = toggle.update(cx, |this, cx| {
                                            this.ensure_question_input(index, window, cx);
                                            if let Some(item) = this.questions.get_mut(index) {
                                                item.expanded = !item.expanded;
                                            }
                                            this.question_list.remeasure_items(index..index + 1);
                                            cx.notify();
                                        });
                                    }),
                            ),
                    ),
            );
        if question.expanded {
            if let Some(input) = &question.title_input {
                card = card.child(Input::new(input).w_full());
            }
            card = card.child(div().text_color(theme.text_secondary).child(format!(
                "{} - {} / {}",
                mezon_i18n::t(locale, "onBoardingClan.questionsPage.addAnswer"),
                question.answers.len(),
                MAX_ANSWERS
            )));
            let mut answers = h_flex().gap_2().flex_wrap();
            for (answer_index, answer) in question.answers.iter().enumerate() {
                let edit = cx.entity().downgrade();
                let title = if answer.title.chars().count() > 42 {
                    format!("{}...", answer.title.chars().take(42).collect::<String>())
                } else {
                    answer.title.clone()
                };
                answers = answers.child(
                    v_flex()
                        .id(format!("answer-{index}-{answer_index}"))
                        .max_w(px(260.0))
                        .p_3()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(theme.border)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg_hover).border_color(theme.text_secondary))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(title),
                        )
                        .when(!answer.description.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_sm()
                                    .text_color(theme.text_secondary)
                                    .truncate()
                                    .child(answer.description.clone()),
                            )
                        })
                        .on_click(move |_, window, cx| {
                            let _ = edit.update(cx, |this, cx| {
                                this.open_answer_editor(index, Some(answer_index), window, cx)
                            });
                        }),
                );
            }
            if question.answers.len() < MAX_ANSWERS {
                answers = answers.child(
                    div()
                        .id(("add-answer", index))
                        .p_3()
                        .rounded(px(8.0))
                        .border_2()
                        .border_dashed()
                        .border_color(theme.border)
                        .cursor_pointer()
                        .text_color(theme.text_primary)
                        .font_weight(FontWeight::BOLD)
                        .hover(|style| style.border_color(theme.text_secondary).bg(theme.bg_hover))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::CirclePlusFill)
                                        .size_5()
                                        .text_color(theme.text_primary),
                                )
                                .child(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.questionsPage.addAnswer",
                                )),
                        )
                        .on_click(move |_, window, cx| {
                            let _ = add_answer.update(cx, |this, cx| {
                                this.open_answer_editor(index, None, window, cx)
                            });
                        }),
                );
            }
            card = card.child(answers).child(
                h_flex().justify_end().child(
                    Self::button(
                        format!("onboarding-question-save-{index}"),
                        mezon_i18n::t(locale, "onBoardingClan.buttons.save"),
                        true,
                        theme,
                    )
                    .disabled(!question_valid)
                    .on_click(move |_, _, cx| {
                        let _ = save.update(cx, |this, cx| {
                            this.sync_question_titles(cx);
                            if let Some(item) = this.questions.get_mut(index)
                                && !item.title.is_empty()
                            {
                                item.expanded = false;
                                this.dirty = true;
                                this.question_list.remeasure_items(index..index + 1);
                            }
                            cx.notify();
                        });
                    }),
                ),
            );
        } else if !question.title.is_empty() {
            card = card.child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(question.title.clone()),
            );
        }
        card.into_any_element()
    }

    fn render_guide(
        &mut self,
        theme: &Theme,
        locale: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let back = cx.entity().downgrade();
        let add_mission = cx.entity().downgrade();
        let add_resource = cx.entity().downgrade();
        let (owner_name, owner_avatar) = self.owner_display(cx);
        let public_channels = self.public_channels(cx);
        if self.mission_list.item_count() != self.missions.len() {
            self.mission_list.reset(self.missions.len());
        }
        if self.resource_list.item_count() != self.resources.len() {
            self.resource_list.reset(self.resources.len());
        }
        let entity = cx.entity();
        let mission_theme = theme.clone();
        let mission_locale = SharedString::from(locale.to_string());
        let mission_channels = public_channels;
        let mission_state = self.mission_list.clone();
        let mission_scroll_state = mission_state.clone();
        let mission_height = (self.missions.len() as f32 * 92.0).min(320.0);
        let missions = div()
            .w_full()
            .h(px(mission_height))
            .on_scroll_wheel(move |event, window, cx| {
                let delta_y = event.delta.pixel_delta(window.line_height()).y;
                if Self::list_can_consume_scroll(&mission_scroll_state, delta_y) {
                    cx.stop_propagation();
                }
            })
            .child(
                list(mission_state, move |index, _, cx| {
                    entity.update(cx, |this, cx| {
                        let mission = &this.missions[index];
                        let channel = mission_channels
                            .iter()
                            .find(|(id, _)| *id == mission.channel_id)
                            .map(|(_, name)| SharedString::from(format!("#{name}")));
                        let edit = cx.entity().downgrade();
                        this.guide_item(
                            format!("mission-row-{index}"),
                            mission.title.clone(),
                            this.task_label(mission.task_type, &mission_locale),
                            channel,
                            Some(IconName::Hashtag),
                            Some((edit, index, false)),
                            &mission_theme,
                        )
                    })
                })
                .size_full(),
            );
        let entity = cx.entity();
        let resource_theme = theme.clone();
        let resource_state = self.resource_list.clone();
        let resource_scroll_state = resource_state.clone();
        let resource_height = (self.resources.len() as f32 * 92.0).min(320.0);
        let resources = div()
            .w_full()
            .h(px(resource_height))
            .on_scroll_wheel(move |event, window, cx| {
                let delta_y = event.delta.pixel_delta(window.line_height()).y;
                if Self::list_can_consume_scroll(&resource_scroll_state, delta_y) {
                    cx.stop_propagation();
                }
            })
            .child(
                list(resource_state, move |index, _, cx| {
                    entity.update(cx, |this, cx| {
                        let resource = &this.resources[index];
                        let edit = cx.entity().downgrade();
                        this.guide_item(
                            format!("resource-row-{index}"),
                            resource.title.clone(),
                            resource.description.clone(),
                            None,
                            Some(IconName::RuleIcon),
                            Some((edit, index, true)),
                            &resource_theme,
                        )
                    })
                })
                .size_full(),
            );
        let is_light = theme_is_light(theme);
        let (welcome_border_start, welcome_border_end, welcome_start, welcome_end) = if is_light {
            (0x8ea8ff, 0xd7b4ff, 0xa9b8ff, 0xd4b4ff)
        } else {
            (0x9e9e9e, 0x494949, 0x777777, 0x4b4b4b)
        };
        v_flex()
            .gap_5()
            .child(
                div()
                    .id("onboarding-guide-back")
                    .cursor_pointer()
                    .text_color(theme.text_secondary)
                    .child(format!(
                        "← {}",
                        mezon_i18n::t(locale, "onBoardingClan.buttons.back")
                    ))
                    .on_click(move |_, _, cx| {
                        let _ = back.update(cx, |this, cx| {
                            this.page = Page::Main;
                            cx.notify();
                        });
                    }),
            )
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.clanGuideSetting.welcomeSign.title",
                    )),
            )
            .child(div().text_color(theme.text_secondary).child(mezon_i18n::t(
                locale,
                "onBoardingClan.clanGuideSetting.welcomeSign.description",
            )))
            .child(
                div()
                    .p(px(2.0))
                    .rounded(px(8.0))
                    .bg(linear_gradient(
                        135.0,
                        linear_color_stop(rgb(welcome_border_start), 0.0),
                        linear_color_stop(rgb(welcome_border_end), 1.0),
                    ))
                    .child(
                        v_flex()
                            .gap_2()
                            .p_4()
                            .rounded(px(6.0))
                            .bg(linear_gradient(
                                135.0,
                                linear_color_stop(rgb(welcome_start), 0.0),
                                linear_color_stop(rgb(welcome_end), 1.0),
                            ))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child({
                                        let mut avatar = Avatar::new()
                                            .name(owner_name.clone())
                                            .size_px(px(48.0));
                                        if let Some(src) = owner_avatar {
                                            avatar = avatar.src(src);
                                        }
                                        avatar
                                    })
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(0xffffff))
                                            .child(owner_name)
                                            .child(
                                                Icon::new(IconName::OwnerIcon)
                                                    .size_4()
                                                    .text_color(rgb(0xfacc15)),
                                            ),
                                    ),
                            )
                            .child(div().text_color(rgb(0xffffff)).child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.clanGuideSetting.ownerGreeting",
                            ))),
                    ),
            )
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.clanGuideSetting.newMemberToDos.title",
                    )),
            )
            .child(self.todo_description(theme, locale))
            .child(div().text_sm().font_weight(FontWeight::BOLD).child(
                mezon_i18n::t(locale, "onBoardingClan.clanGuideSetting.dontDoThis").to_uppercase(),
            ))
            .child(self.guide_example_item(theme, locale))
            .when(!self.missions.is_empty(), |page| page.child(missions))
            .child(self.guide_item(
                "read-rules-row",
                mezon_i18n::t(locale, "onBoardingClan.clanGuideSetting.readTheRules"),
                "",
                None,
                Some(IconName::RuleIcon),
                None,
                theme,
            ))
            .child(
                div()
                    .id("add-onboarding-mission")
                    .p_4()
                    .rounded(px(8.0))
                    .border_2()
                    .border_dashed()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .text_center()
                    .text_color(rgb(0x949cf7))
                    .hover(|style| style.border_color(theme.text_secondary).bg(theme.bg_hover))
                    .child(
                        h_flex()
                            .justify_center()
                            .gap_2()
                            .font_weight(FontWeight::BOLD)
                            .child(
                                Icon::new(IconName::CirclePlusFill)
                                    .size_5()
                                    .text_color(rgb(0x949cf7)),
                            )
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.clanGuideSetting.addTask",
                            )),
                    )
                    .on_click(move |_, window, cx| {
                        let _ = add_mission
                            .update(cx, |this, cx| this.open_mission_editor(None, window, cx));
                    }),
            )
            .child(div().mt_4().border_t_1().border_color(theme.border))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.clanGuideSetting.resourcePages.title",
                    )),
            )
            .child(div().text_color(theme.text_secondary).child(mezon_i18n::t(
                locale,
                "onBoardingClan.clanGuideSetting.resourcePages.description",
            )))
            .child(
                v_flex()
                    .gap_1()
                    .text_color(theme.text_secondary)
                    .child(format!(
                        "• {}",
                        mezon_i18n::t(
                            locale,
                            "onBoardingClan.clanGuideSetting.resourcePages.perk1"
                        )
                    ))
                    .child(format!(
                        "• {}",
                        mezon_i18n::t(
                            locale,
                            "onBoardingClan.clanGuideSetting.resourcePages.perk2"
                        )
                    ))
                    .child(format!(
                        "• {}",
                        mezon_i18n::t(
                            locale,
                            "onBoardingClan.clanGuideSetting.resourcePages.perk3"
                        )
                    )),
            )
            .when(!self.resources.is_empty(), |page| page.child(resources))
            .child(
                div()
                    .id("add-onboarding-resource")
                    .p_4()
                    .rounded(px(8.0))
                    .border_2()
                    .border_dashed()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .text_center()
                    .text_color(rgb(0x949cf7))
                    .hover(|style| style.border_color(theme.text_secondary).bg(theme.bg_hover))
                    .child(
                        h_flex()
                            .justify_center()
                            .gap_2()
                            .font_weight(FontWeight::BOLD)
                            .child(
                                Icon::new(IconName::CirclePlusFill)
                                    .size_5()
                                    .text_color(rgb(0x949cf7)),
                            )
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.clanGuideSetting.addResource",
                            )),
                    )
                    .on_click(move |_, window, cx| {
                        let _ = add_resource
                            .update(cx, |this, cx| this.open_resource_editor(None, window, cx));
                    }),
            )
            .into_any_element()
    }

    fn guide_item(
        &self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        channel_name: Option<SharedString>,
        icon: Option<IconName>,
        edit_action: Option<(gpui::WeakEntity<Self>, usize, bool)>,
        theme: &Theme,
    ) -> gpui::AnyElement {
        let description = description.into();
        let has_description = !description.is_empty();
        h_flex()
            .id(id.into())
            .w_full()
            .items_center()
            .justify_between()
            .p_4()
            .rounded(px(8.0))
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_secondary)
            .hover(|style| style.bg(theme.bg_hover).border_color(theme.text_secondary))
            .when_some(icon, |row, icon| {
                row.child(
                    div()
                        .w(px(40.0))
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .child(
                            Icon::new(icon)
                                .size(px(20.0))
                                .text_color(theme.text_secondary),
                        ),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .items_start()
                    .child(
                        div()
                            .w_full()
                            .text_left()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title.into()),
                    )
                    .when(has_description, |column| {
                        column.child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .gap_1()
                                .text_sm()
                                .text_color(theme.text_secondary)
                                .child(div().truncate().child(description))
                                .when_some(channel_name, |row, channel| {
                                    row.child(
                                        div()
                                            .flex_shrink_0()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.text_primary)
                                            .child(channel),
                                    )
                                }),
                        )
                    }),
            )
            .when_some(edit_action, |row, (edit, index, is_resource)| {
                row.child(
                    div()
                        .id(if is_resource {
                            format!("edit-resource-button-{index}")
                        } else {
                            format!("edit-mission-button-{index}")
                        })
                        .size(px(36.0))
                        .flex_shrink_0()
                        .rounded(px(6.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgb(0x5865f2))
                        .hover(|style| style.bg(rgb(0x4752c4)))
                        .child(
                            Icon::new(IconName::PenEdit)
                                .size_5()
                                .text_color(rgb(0xffffff)),
                        )
                        .on_click(move |_, window, cx| {
                            let _ = edit.update(cx, |this, cx| {
                                if is_resource {
                                    this.open_resource_editor(Some(index), window, cx);
                                } else {
                                    this.open_mission_editor(Some(index), window, cx);
                                }
                            });
                        }),
                )
            })
            .into_any_element()
    }

    fn guide_example_item(&self, theme: &Theme, locale: &str) -> gpui::AnyElement {
        h_flex()
            .gap_4()
            .p_4()
            .rounded(px(8.0))
            .border_2()
            .border_color(theme.border)
            .bg(theme.bg_secondary)
            .child(
                Icon::new(IconName::IconRemove)
                    .size_5()
                    .text_color(theme.text_secondary),
            )
            .child(
                v_flex()
                    .child(div().font_weight(FontWeight::BOLD).child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.clanGuideSetting.exampleTask.title",
                    )))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_secondary)
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.clanGuideSetting.exampleTask.description",
                            )),
                    ),
            )
            .into_any_element()
    }

    fn todo_description(&self, theme: &Theme, locale: &str) -> gpui::AnyElement {
        let value = mezon_i18n::t(
            locale,
            "onBoardingClan.clanGuideSetting.newMemberToDos.description",
        );
        let (prefix, emphasized) = value
            .split_once("<strong>")
            .and_then(|(prefix, rest)| {
                rest.split_once("</strong>")
                    .map(|(emphasized, _)| (prefix, emphasized))
            })
            .unwrap_or((value, ""));
        h_flex()
            .flex_wrap()
            .text_color(theme.text_secondary)
            .child(prefix.to_string())
            .when(!emphasized.is_empty(), |row| {
                row.child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(emphasized.to_string()),
                )
            })
            .into_any_element()
    }

    fn owner_display(&self, cx: &App) -> (SharedString, Option<SharedString>) {
        let Some(clan) = self.clan_list.read(cx).clan(self.clan_id) else {
            return (SharedString::default(), None);
        };
        let Some(member) = ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, clan.creator_id)
        else {
            return (SharedString::from(clan.name.clone()), None);
        };
        let name = SharedString::from(member.name().to_string());
        let avatar = (!member.avatar().is_empty())
            .then(|| SharedString::from(crate::util::imgproxy::avatar_url(cx, member.avatar())));
        (name, avatar)
    }

    fn task_label(&self, task_type: i32, locale: &str) -> SharedString {
        let key = match task_type {
            TASK_VISIT => "onboardingMissions.missions.visit.description",
            TASK_READ => "onboardingMissions.missions.doSomething.summary",
            _ => "onboardingMissions.missions.sendMessage.description",
        };
        mezon_i18n::t(locale, key).into()
    }

    fn task_form_label(&self, task_type: i32, locale: &str) -> SharedString {
        let key = match task_type {
            TASK_VISIT => "onboardingMissions.missions.visit.description",
            TASK_READ => "onboardingMissions.missions.doSomething.description",
            _ => "onboardingMissions.missions.sendMessage.description",
        };
        mezon_i18n::t(locale, key).into()
    }

    fn render_editor(
        &self,
        entity: gpui::WeakEntity<Self>,
        theme: &Theme,
        locale: &str,
        cx: &App,
    ) -> Option<gpui::AnyElement> {
        let editor = self.editor.as_ref()?;
        let editor_valid = match editor {
            Editor::Answer { title, .. } => !title.read(cx).value().trim().is_empty(),
            Editor::Mission {
                title, channel_id, ..
            } => title.read(cx).value().trim().chars().count() >= 7 && *channel_id != 0,
            Editor::Resource { title, .. } => title.read(cx).value().trim().chars().count() >= 7,
        };
        let editor_is_new = match editor {
            Editor::Answer { answer, .. } => answer.is_none(),
            Editor::Mission { index, .. } | Editor::Resource { index, .. } => index.is_none(),
        };
        let close = entity.clone();
        let cancel = entity.clone();
        let save = entity.clone();
        let remove = entity.clone();
        let body = match editor {
            Editor::Answer {
                question,
                title,
                description,
                ..
            } => {
                let question_title = self.questions[*question]
                    .title_input
                    .as_ref()
                    .map(|input| input.read(cx).value().trim().to_string())
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        let saved = self.questions[*question].title.clone();
                        (!saved.is_empty()).then_some(saved)
                    })
                    .unwrap_or_else(|| {
                        mezon_i18n::t(locale, "onBoardingClan.questionsPage.questionPlaceholder")
                            .to_string()
                    });
                v_flex()
                    .gap_4()
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div().text_xs().text_color(theme.text_secondary).child(
                                    format!(
                                        "{} {}",
                                        mezon_i18n::t(locale, "onBoardingClan.questionsPage.title"),
                                        *question + 1
                                    )
                                    .to_uppercase(),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(22.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(format!("{question_title} ?")),
                            ),
                    )
                    .child(self.field(
                        mezon_i18n::t(locale, "onBoardingClan.questionsPage.answerTitle"),
                        true,
                        Input::new(title),
                        theme,
                    ))
                    .child(self.field(
                        mezon_i18n::t(locale, "onBoardingClan.questionsPage.answerDescription"),
                        false,
                        Input::new(description),
                        theme,
                    ))
                    .into_any_element()
            }
            Editor::Mission {
                title,
                channel_id,
                task_type,
                channels_open,
                ..
            } => {
                let toggle = entity.clone();
                let selected = self
                    .public_channels(cx)
                    .into_iter()
                    .find(|(id, _)| id == channel_id)
                    .map(|(_, name)| name)
                    .unwrap_or_default();
                let mut channels = v_flex()
                    .id("onboarding-channel-options")
                    .absolute()
                    .top_full()
                    .left_0()
                    .right_0()
                    .max_h(px(180.0))
                    .overflow_y_scroll()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme.text_muted)
                    .bg(theme.bg_secondary)
                    .shadow_lg();
                if *channels_open {
                    for (id, name) in self.public_channels(cx) {
                        let select = entity.clone();
                        channels = channels.child(
                            div()
                                .id(("mission-channel", id as u64))
                                .p_2()
                                .bg(theme.bg_secondary)
                                .cursor_pointer()
                                .hover(|style| {
                                    style.bg(theme.bg_hover).text_color(theme.text_primary)
                                })
                                .child(name)
                                .on_click(move |_, _, cx| {
                                    let _ = select.update(cx, |this, cx| {
                                        if let Some(Editor::Mission {
                                            channel_id,
                                            channels_open,
                                            ..
                                        }) = &mut this.editor
                                        {
                                            *channel_id = id;
                                            *channels_open = false;
                                        }
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                }
                let mut radios = v_flex().gap_2();
                for kind in [TASK_SEND_MESSAGE, TASK_VISIT, TASK_READ] {
                    let select = entity.clone();
                    radios = radios.child(
                        h_flex()
                            .id(("mission-type", kind as u32))
                            .gap_2()
                            .cursor_pointer()
                            .child(if *task_type == kind { "◉" } else { "○" })
                            .child(self.task_form_label(kind, locale))
                            .on_click(move |_, _, cx| {
                                let _ = select.update(cx, |this, cx| {
                                    if let Some(Editor::Mission { task_type, .. }) =
                                        &mut this.editor
                                    {
                                        *task_type = kind;
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                let title_field = self
                    .field(
                        mezon_i18n::t(locale, "onboardingMissions.form.title"),
                        true,
                        Input::new(title),
                        theme,
                    )
                    .into_any_element();
                let channel_select = v_flex()
                    .id("onboarding-channel-select")
                    .relative()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme.text_muted)
                    .bg(theme.bg_secondary)
                    .child(
                        h_flex()
                            .id("onboarding-channel-trigger")
                            .p_3()
                            .justify_between()
                            .cursor_pointer()
                            .child(selected)
                            .child(Icon::new(IconName::ChevronDown).size_4())
                            .on_click(move |_, _, cx| {
                                let _ = toggle.update(cx, |this, cx| {
                                    if let Some(Editor::Mission { channels_open, .. }) =
                                        &mut this.editor
                                    {
                                        *channels_open = !*channels_open;
                                    }
                                    cx.notify();
                                });
                            }),
                    )
                    .when(*channels_open, |select| select.child(deferred(channels)))
                    .into_any_element();
                onboarding_modal::mission_editor_body(
                    title_field,
                    mezon_i18n::t(locale, "onboardingMissions.form.channelTitle").into(),
                    channel_select,
                    mezon_i18n::t(locale, "onboardingMissions.form.channelNote").into(),
                    mezon_i18n::t(locale, "onboardingMissions.form.completeWhen").into(),
                    radios.into_any_element(),
                    theme,
                )
            }
            Editor::Resource {
                index,
                title,
                description,
                image_url,
                image_path,
                ..
            } => {
                let browse = entity.clone();
                let preview = image_path
                    .as_ref()
                    .map(|path| {
                        img(path.clone())
                            .w(px(40.0))
                            .h(px(40.0))
                            .rounded(px(6.0))
                            .object_fit(gpui::ObjectFit::Cover)
                            .into_any_element()
                    })
                    .or_else(|| {
                        (!image_url.is_empty()).then(|| {
                            img(image_url.clone())
                                .w(px(40.0))
                                .h(px(40.0))
                                .rounded(px(6.0))
                                .object_fit(gpui::ObjectFit::Cover)
                                .into_any_element()
                        })
                    });
                let title_field = self
                    .field(
                        mezon_i18n::t(locale, "onboardingRules.form.nameTitle"),
                        true,
                        Input::new(title),
                        theme,
                    )
                    .into_any_element();
                let description_field = self
                    .field(
                        mezon_i18n::t(locale, "onboardingRules.form.descriptionTitle"),
                        false,
                        Input::new(description),
                        theme,
                    )
                    .into_any_element();
                let browse_button = Self::button(
                    "onboarding-resource-browse",
                    mezon_i18n::t(locale, "onboardingRules.form.browse"),
                    true,
                    theme,
                )
                .on_click(move |_, _, cx| {
                    let _ = browse.update(cx, |this, cx| this.pick_resource_image(cx));
                })
                .into_any_element();
                onboarding_modal::resource_editor_body(
                    if index.is_some() {
                        mezon_i18n::t(locale, "onboardingRules.editResources").into()
                    } else {
                        mezon_i18n::t(locale, "onBoardingClan.clanGuideSetting.addResource").into()
                    },
                    title_field,
                    description_field,
                    mezon_i18n::t(locale, "onboardingRules.form.imageTitle").into(),
                    browse_button,
                    preview,
                    theme,
                )
            }
        };
        let footer = h_flex()
            .flex_shrink_0()
            .justify_between()
            .p_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("remove-onboarding-editor-item")
                    .cursor_pointer()
                    .text_color(theme.danger_text)
                    .child(mezon_i18n::t(
                        locale,
                        if editor_is_new {
                            "common.reset"
                        } else {
                            "onBoardingClan.questionsPage.remove"
                        },
                    ))
                    .on_click(move |_, window, cx| {
                        let _ = remove.update(cx, |this, cx| {
                            if editor_is_new {
                                this.reset_editor(window, cx);
                            } else {
                                this.remove_editor_item(cx);
                            }
                        });
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Self::button(
                            "onboarding-editor-cancel",
                            mezon_i18n::t(locale, "onBoardingClan.buttons.cancel"),
                            false,
                            theme,
                        )
                        .on_click(move |_, _, cx| {
                            let _ = cancel.update(cx, |this, cx| {
                                this.editor_generation = this.editor_generation.wrapping_add(1);
                                this.editor = None;
                                cx.notify();
                            });
                        }),
                    )
                    .child(
                        Self::button(
                            "onboarding-editor-save",
                            mezon_i18n::t(locale, "onBoardingClan.buttons.save"),
                            true,
                            theme,
                        )
                        .disabled(!editor_valid)
                        .on_click(move |_, _, cx| {
                            let _ = save.update(cx, |this, cx| this.commit_editor(cx));
                        }),
                    ),
            )
            .into_any_element();
        Some(onboarding_modal::editor_modal(
            body,
            footer,
            theme,
            move |_, _, cx| {
                let _ = close.update(cx, |this, cx| {
                    this.editor_generation = this.editor_generation.wrapping_add(1);
                    this.editor = None;
                    cx.notify();
                });
            },
        ))
    }

    fn field(
        &self,
        label: impl Into<SharedString>,
        required: bool,
        input: Input,
        theme: &Theme,
    ) -> gpui::Div {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(label.into()))
                    .when(required, |el| {
                        el.child(div().text_color(theme.danger_text).child("*"))
                    }),
            )
            .child(input.w_full())
    }

    fn commit_editor(&mut self, cx: &mut Context<Self>) {
        let valid = match self.editor.as_ref() {
            Some(Editor::Answer { title, .. }) => !title.read(cx).value().trim().is_empty(),
            Some(Editor::Mission {
                title, channel_id, ..
            }) => title.read(cx).value().trim().chars().count() >= 7 && *channel_id != 0,
            Some(Editor::Resource { title, .. }) => {
                title.read(cx).value().trim().chars().count() >= 7
            }
            None => false,
        };
        if !valid {
            cx.notify();
            return;
        }
        let mut applied = false;
        match self.editor.take() {
            Some(Editor::Answer {
                question,
                answer,
                title,
                description,
            }) => {
                let title = title.read(cx).value().trim().to_string();
                let value = AnswerDraft {
                    title,
                    description: description.read(cx).value().trim().to_string(),
                };
                if let Some(question_draft) = self.questions.get_mut(question) {
                    if let Some(index) = answer {
                        if let Some(answer) = question_draft.answers.get_mut(index) {
                            *answer = value;
                            applied = true;
                        }
                    } else {
                        question_draft.answers.push(value);
                        applied = true;
                    }
                    self.question_list
                        .remeasure_items(question..question.saturating_add(1));
                }
            }
            Some(Editor::Mission {
                index,
                title,
                channel_id,
                task_type,
                channels_open: _,
            }) => {
                let title = title.read(cx).value().trim().to_string();
                let value = MissionDraft {
                    id: index
                        .and_then(|i| self.missions.get(i))
                        .and_then(|item| item.id),
                    title,
                    channel_id,
                    task_type,
                };
                if let Some(index) = index {
                    if let Some(item) = self.missions.get_mut(index) {
                        *item = value;
                        applied = true;
                    }
                } else {
                    self.missions.push(value);
                    applied = true;
                }
            }
            Some(Editor::Resource {
                index,
                title,
                description,
                image_url,
                image_path,
            }) => {
                let title = title.read(cx).value().trim().to_string();
                let value = ResourceDraft {
                    id: index
                        .and_then(|i| self.resources.get(i))
                        .and_then(|item| item.id),
                    title,
                    description: description.read(cx).value().trim().to_string(),
                    image_url,
                    image_path,
                };
                if let Some(index) = index {
                    if let Some(item) = self.resources.get_mut(index) {
                        *item = value;
                        applied = true;
                    }
                } else {
                    self.resources.push(value);
                    applied = true;
                }
            }
            None => return,
        }
        self.editor_generation = self.editor_generation.wrapping_add(1);
        if applied {
            self.dirty = true;
        }
        cx.notify();
    }

    fn remove_editor_item(&mut self, cx: &mut Context<Self>) {
        let mut removed = false;
        match self.editor.take() {
            Some(Editor::Answer {
                question,
                answer: Some(answer),
                ..
            }) => {
                if let Some(question_draft) = self.questions.get_mut(question)
                    && answer < question_draft.answers.len()
                {
                    question_draft.answers.remove(answer);
                    removed = true;
                    self.question_list
                        .remeasure_items(question..question.saturating_add(1));
                }
            }
            Some(Editor::Mission {
                index: Some(index), ..
            }) if index < self.missions.len() => {
                self.missions.remove(index);
                removed = true;
            }
            Some(Editor::Resource {
                index: Some(index), ..
            }) if index < self.resources.len() => {
                self.resources.remove(index);
                removed = true;
            }
            _ => {}
        }
        self.editor_generation = self.editor_generation.wrapping_add(1);
        if removed {
            self.dirty = true;
        }
        cx.notify();
    }

    fn reset_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_generation = self.editor_generation.wrapping_add(1);
        let default_channel = self
            .public_channels(cx)
            .first()
            .map(|(id, _)| *id)
            .unwrap_or(0);
        match self.editor.as_mut() {
            Some(Editor::Answer {
                title, description, ..
            }) => {
                title.update(cx, |state, cx| state.set_value("", window, cx));
                description.update(cx, |state, cx| state.set_value("", window, cx));
            }
            Some(Editor::Mission {
                title,
                channel_id,
                task_type,
                channels_open,
                ..
            }) => {
                title.update(cx, |state, cx| state.set_value("", window, cx));
                *channel_id = default_channel;
                *task_type = TASK_SEND_MESSAGE;
                *channels_open = false;
            }
            Some(Editor::Resource {
                title,
                description,
                image_url,
                image_path,
                ..
            }) => {
                title.update(cx, |state, cx| state.set_value("", window, cx));
                description.update(cx, |state, cx| state.set_value("", window, cx));
                image_url.clear();
                *image_path = None;
            }
            None => return,
        }
        cx.notify();
    }

    fn pick_resource_image(&mut self, cx: &mut Context<Self>) {
        let Some(Editor::Resource { index, .. }) = self.editor.as_ref() else {
            return;
        };
        let target_index = *index;
        let generation = self.editor_generation;
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        self._upload_task = Some(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let upload = this
                .update(cx, |this, cx| {
                    this.clan_list.update(cx, |store, cx| {
                        store.upload_clan_image(&path, 10_000_000, cx)
                    })
                })
                .ok();
            let Some(upload) = upload else {
                return;
            };
            let result = upload.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(url) => {
                        if this.editor_generation == generation
                            && let Some(Editor::Resource {
                                index,
                                image_url,
                                image_path,
                                ..
                            }) = &mut this.editor
                            && *index == target_index
                        {
                            *image_url = url;
                            *image_path = Some(path);
                        }
                    }
                    Err(error) => Shell::global(cx).update(cx, |shell, cx| shell.error(error, cx)),
                }
                cx.notify();
            });
        }));
    }
}

impl Render for OnboardingSettingPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.locale(cx);
        if self.loading {
            return div()
                .py_8()
                .text_color(theme.text_secondary)
                .child(mezon_i18n::t(&locale, "common.loadingData"))
                .into_any_element();
        }
        if let Some(error) = self.load_error.clone() {
            let retry = cx.entity().downgrade();
            return v_flex()
                .items_center()
                .gap_3()
                .py_8()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.danger_text)
                        .child(mezon_i18n::t(&locale, "common.failedToLoad")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.text_secondary)
                        .child(error),
                )
                .child(
                    Self::button(
                        "onboarding-retry-load",
                        mezon_i18n::t(&locale, "common.errorBoundary.reload"),
                        true,
                        &theme,
                    )
                    .on_click(move |_, _, cx| {
                        let _ = retry.update(cx, |this, cx| this.fetch(cx));
                    }),
                )
                .into_any_element();
        }
        if !self.enabled && !self.setup_open {
            return Self::render_enable_card(cx.entity(), &locale, &theme);
        }
        let content = match self.page {
            Page::Main => self.render_main(&theme, &locale, cx),
            Page::Questions => self.render_questions(&theme, &locale, cx),
            Page::Guide => self.render_guide(&theme, &locale, cx),
        };
        let disable = cx.entity().downgrade();
        let cancel = cx.entity().downgrade();
        let close_setup = cx.entity().downgrade();
        let confirm = cx.entity().downgrade();
        let page = v_flex()
            .gap_4()
            .when(!self.setup_open, |page| {
                page.child(
                    h_flex()
                        .justify_between()
                        .items_start()
                        .child(
                            v_flex()
                                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(
                                    mezon_i18n::t(&locale, "onBoardingClan.onboarding.title"),
                                ))
                                .child(div().text_sm().text_color(theme.text_secondary).child(
                                    mezon_i18n::t(
                                        &locale,
                                        "onBoardingClan.onboarding.featuresEnabled",
                                    ),
                                )),
                        )
                        .when(self.enabled, |el| {
                            el.child(
                                Self::button(
                                    "onboarding-disable",
                                    mezon_i18n::t(&locale, "onBoardingClan.buttons.disable"),
                                    false,
                                    &theme,
                                )
                                .with_variant(crate::components::primitives::ButtonVariant::Danger)
                                .on_click(move |_, _, cx| {
                                    let _ = disable.update(cx, |this, cx| this.disable(cx));
                                }),
                            )
                        }),
                )
            })
            .child(content);
        if self.setup_open {
            let modal_height = if self.page == Page::Main {
                480.0
            } else {
                650.0
            };
            let body = div()
                .id("onboarding-setup-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_5()
                .pb_4()
                .child(page)
                .into_any_element();
            let footer = h_flex()
                .flex_shrink_0()
                .justify_end()
                .gap_3()
                .p_4()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    Self::button(
                        "onboarding-setup-cancel",
                        mezon_i18n::t(&locale, "onBoardingClan.buttons.cancel"),
                        false,
                        &theme,
                    )
                    .on_click(move |_, _, cx| {
                        let _ = cancel.update(cx, |this, cx| this.cancel_setup(cx));
                    }),
                )
                .child(
                    Self::button(
                        "onboarding-setup-confirm",
                        if self.saving {
                            mezon_i18n::t(&locale, "onBoardingClan.buttons.saving")
                        } else {
                            mezon_i18n::t(&locale, "onBoardingClan.buttons.confirm")
                        },
                        true,
                        &theme,
                    )
                    .on_click(move |_, _, cx| {
                        let _ = confirm.update(cx, |this, cx| {
                            if !this.saving {
                                this.save_all(true, cx);
                            }
                        });
                    }),
                )
                .into_any_element();
            return onboarding_modal::setup_modal(
                mezon_i18n::t(&locale, "onBoardingClan.modal.title").into(),
                body,
                footer,
                None,
                760.0,
                modal_height,
                &theme,
                move |_, _, cx| {
                    let _ = close_setup.update(cx, |this, cx| this.cancel_setup(cx));
                },
            );
        }
        page.into_any_element()
    }
}

pub fn render_onboarding_editor_modal(
    page: Entity<OnboardingSettingPage>,
    locale: &str,
    theme: &Theme,
    cx: &mut App,
) -> gpui::AnyElement {
    page.read(cx)
        .render_editor(page.downgrade(), theme, locale, cx)
        .unwrap_or_else(|| div().into_any_element())
}

pub fn render_onboarding_save_bar(
    page: Entity<OnboardingSettingPage>,
    locale: &str,
    _theme: &Theme,
    cx: &mut App,
) -> gpui::AnyElement {
    let saving = page.read(cx).saving;
    let reset = page.clone();
    let save = page.clone();
    UnsavedChangesBar::new(
        mezon_i18n::t(locale, "clanSettings.modalSaveChanges.title"),
        Button::new("onboarding-reset")
            .label(mezon_i18n::t(locale, "clanSettings.modalSaveChanges.reset"))
            .ghost()
            .on_click(move |_, _, cx| {
                reset.update(cx, |this, cx| this.reset_changes(cx));
            }),
        Button::new("onboarding-save")
            .label(mezon_i18n::t(
                locale,
                "clanSettings.modalSaveChanges.saveChanges",
            ))
            .primary()
            .disabled(saving)
            .on_click(move |_, _, cx| {
                save.update(cx, |this, cx| this.save_all(false, cx));
            }),
    )
    .into_any_element()
}
