use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, FontWeight, InteractiveElement, ObjectFit,
    Render, SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div, img,
    prelude::*, px, relative, rgb,
};
use mezon_store::{
    ChannelId, ChannelList, ClanId, ClanList, ClanMembersStore, MISSION_DO_SOMETHING,
    MISSION_SEND_MESSAGE, MISSION_VISIT, MessagesStore, OnboardingItem, OnboardingStore, Settings,
};

use crate::chat::clan_management_page::management_page;
use crate::clan::invite_people_modal::open_invite_people_modal;
use crate::components::primitives::{Icon, IconName, h_flex, v_flex};
use crate::theme::{ActiveTheme, Theme};
use crate::util::imgproxy::proxied;
use crate::util::theme::theme_is_light;

const TICK_COLOR: u32 = 0x40c174;
const PROGRESS_COLOR: u32 = 0x16a34a;
const CHIP_BORDER_LIGHT: u32 = 0xe5e7eb;
const CHIP_BORDER_DARK: u32 = 0x374151;
const LOGO_PLACEHOLDER: u32 = 0x09090b;

enum ContentState {
    Loading,
    Failed,
    Ready,
}

pub struct ClanGuidePage {
    clan_id: ClanId,
    settings: Entity<Settings>,
}

impl ClanGuidePage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&OnboardingStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.observe(&ClanList::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.observe(&ChannelList::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.observe(&ClanMembersStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self {
            clan_id: ClanId(0),
            settings,
        }
    }

    pub fn set_clan(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let switched = self.clan_id != clan_id;
        self.clan_id = clan_id;
        let store = OnboardingStore::global(cx);
        let needs_load = {
            let store = store.read(cx);
            !store.has_onboarding(clan_id) && !store.is_loading(clan_id)
        };
        if switched || needs_load {
            store.update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
        }
        if switched {
            cx.notify();
        }
    }

    fn content_state(&self, cx: &App) -> ContentState {
        let store = OnboardingStore::global(cx);
        let store = store.read(cx);
        if store.onboarding(self.clan_id).is_some() {
            return ContentState::Ready;
        }
        if store.load_failed(self.clan_id) {
            return ContentState::Failed;
        }
        ContentState::Loading
    }

    fn clan_name(&self, locale: &str, cx: &App) -> String {
        let clans = ClanList::global(cx);
        let clans = clans.read(cx);
        let Some(clan) = clans.clan(self.clan_id) else {
            return String::new();
        };
        if !clan.name.is_empty() {
            return clan.name.clone();
        }
        let members = ClanMembersStore::global(cx);
        let owner = members
            .read(cx)
            .member(self.clan_id, clan.creator_id)
            .map(|member| {
                if member.user.display_name.is_empty() {
                    member.user.username.clone()
                } else {
                    member.user.display_name.clone()
                }
            })
            .unwrap_or_default();
        format!("{owner}'s {}", mezon_i18n::t(locale, "guide.clan"))
    }

    fn banner_url(&self, cx: &App) -> String {
        let clans = ClanList::global(cx);
        let clans = clans.read(cx);
        clans
            .clan(self.clan_id)
            .and_then(|clan| clan.banner_url.clone())
            .unwrap_or_default()
    }

    fn logo_url(&self, cx: &App) -> String {
        let clans = ClanList::global(cx);
        let clans = clans.read(cx);
        clans
            .clan(self.clan_id)
            .and_then(|clan| clan.avatar_url.clone())
            .unwrap_or_default()
    }

    fn render_banner(&self, theme: &Theme, cx: &App) -> AnyElement {
        let banner = self.banner_url(cx);
        div()
            .flex()
            .flex_col()
            .w(relative(1.04))
            .flex_none()
            .child(
                div()
                    .h(px(144.))
                    .w_full()
                    .rounded(px(12.))
                    .overflow_hidden()
                    .when(banner.is_empty(), |element| {
                        element.bg(theme.tokens.private_theme)
                    })
                    .when(!banner.is_empty(), |element| {
                        element.child(
                            img(proxied(cx, &banner, 1200, 288, "fill"))
                                .w_full()
                                .h_full()
                                .object_fit(ObjectFit::Cover),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_header(&self, theme: &Theme, locale: &str, cx: &App) -> AnyElement {
        let logo = self.logo_url(cx);
        let clan_name = self.clan_name(locale, cx);
        let initial: SharedString = clan_name
            .chars()
            .next()
            .map(String::from)
            .unwrap_or_default()
            .into();
        let clan_id = self.clan_id;
        let invite_name = clan_name.clone();
        let invite_avatar = logo.clone();
        let invite_locale = locale.to_string();
        div()
            .flex()
            .flex_col()
            .w_full()
            .flex_none()
            .relative()
            .justify_end()
            .pt_2()
            .child(
                div()
                    .absolute()
                    .top(px(-48.))
                    .left_0()
                    .size(px(112.))
                    .rounded(px(24.))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(logo.is_empty(), |element| {
                        element
                            .bg(rgb(LOGO_PLACEHOLDER))
                            .text_size(px(36.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.tokens.text_secondary)
                            .child(initial)
                    })
                    .when(!logo.is_empty(), |element| {
                        element.child(
                            img(proxied(cx, &logo, 224, 224, "fill"))
                                .size_full()
                                .object_fit(ObjectFit::Cover),
                        )
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_end()
                    .h(px(112.))
                    .child(
                        div()
                            .flex_shrink_1()
                            .text_size(px(32.))
                            .font_weight(FontWeight::BOLD)
                            .line_height(px(32.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(clan_name),
                    )
                    .child(
                        div()
                            .relative()
                            .size(px(24.))
                            .flex_none()
                            .child(
                                Icon::new(IconName::ClanGuideSeal)
                                    .absolute()
                                    .size(px(24.))
                                    .text_color(theme.tokens.text_theme_primary),
                            )
                            .child(
                                Icon::new(IconName::ClanGuideHouse)
                                    .absolute()
                                    .top(px(4.))
                                    .right(px(4.))
                                    .size(px(16.))
                                    .text_color(theme.tokens.text_secondary),
                            ),
                    )
                    .child(
                        div().flex_1().flex().justify_end().child(
                            div()
                                .id("clan-guide-invite")
                                .w(px(96.))
                                .h(px(36.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(theme.tokens.border_primary)
                                .bg(theme.tokens.bg_tertiary)
                                .text_color(theme.tokens.text_theme_primary)
                                .cursor_pointer()
                                .hover(|style| {
                                    style
                                        .bg(theme.tokens.bg_secondary_button_hover)
                                        .text_color(theme.tokens.text_secondary)
                                })
                                .child(mezon_i18n::t(locale, "guide.invite"))
                                .on_click(move |_, window, cx| {
                                    open_invite_people_modal(
                                        clan_id,
                                        invite_name.clone(),
                                        invite_avatar.clone(),
                                        invite_locale.clone(),
                                        window,
                                        cx,
                                    );
                                }),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_questions(&self, theme: &Theme, locale: &str, cx: &App) -> AnyElement {
        let store = OnboardingStore::global(cx);
        let store = store.read(cx);
        let questions = store
            .onboarding(self.clan_id)
            .map(|onboarding| onboarding.questions.as_slice())
            .unwrap_or_default();
        let section = v_flex()
            .gap_2()
            .child(section_title(mezon_i18n::t(locale, "guide.questions")));
        if questions.is_empty() {
            return section
                .child(empty_card(
                    theme,
                    mezon_i18n::t(locale, "guide.noQuestions"),
                ))
                .into_any_element();
        }
        let mut list = v_flex().gap_2().rounded(px(8.)).relative();
        for question in questions {
            list = list.child(self.render_question(question, theme, cx));
        }
        let fill = (store.answered_percent(self.clan_id) / 100.).clamp(0., 1.);
        section
            .child(
                list.child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(-16.))
                        .w(px(4.))
                        .h_full()
                        .child(
                            div()
                                .relative()
                                .w(px(4.))
                                .h_full()
                                .rounded(px(16.))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .w(px(4.))
                                        .h(relative(fill))
                                        .rounded(px(16.))
                                        .bg(rgb(PROGRESS_COLOR)),
                                ),
                        ),
                ),
            )
            .into_any_element()
    }

    fn render_question(&self, question: &OnboardingItem, theme: &Theme, cx: &App) -> AnyElement {
        let light = theme_is_light(theme);
        let store = OnboardingStore::global(cx);
        let store = store.read(cx);
        let clan_id = self.clan_id;
        let question_id = question.id;
        let mut answers = div().flex().flex_wrap().gap_2().flex_1();
        for (index, answer) in question.answers.iter().enumerate() {
            let selected = store.answer_selected(clan_id, question_id, index);
            let mut content = v_flex().min_w_0().justify_center();
            if !answer.title.is_empty() {
                content = content.child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.tokens.text_secondary)
                        .child(answer.title.clone()),
                );
            }
            if !answer.description.is_empty() {
                content = content.child(div().text_size(px(12.)).child(answer.description.clone()));
            }
            let chip = h_flex()
                .id(SharedString::from(format!(
                    "clan-guide-answer-{question_id}-{index}"
                )))
                .gap_2()
                .items_center()
                .justify_center()
                .px_4()
                .py_2()
                .rounded(px(12.))
                .border_2()
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .map(|chip| {
                    if selected {
                        chip.bg(theme.tokens.bg_active_member_channel)
                            .text_color(theme.tokens.text_secondary)
                            .border_color(theme.tokens.border_primary)
                    } else {
                        chip.when(light, |element| element.bg(gpui::white()))
                            .text_color(theme.tokens.text_theme_primary)
                            .border_color(rgb(if light {
                                CHIP_BORDER_LIGHT
                            } else {
                                CHIP_BORDER_DARK
                            }))
                    }
                })
                .hover(|style| {
                    style
                        .bg(theme.tokens.bg_active_member_channel)
                        .border_color(theme.tokens.border_primary)
                })
                .when(!answer.emoji.is_empty(), |chip| {
                    chip.child(div().flex_none().child(answer.emoji.clone()))
                })
                .child(content)
                .on_click(move |_, _, cx| {
                    OnboardingStore::global(cx).update(cx, |store, cx| {
                        store.toggle_answer(clan_id, question_id, index, cx)
                    });
                });
            answers = answers.child(chip);
        }
        div()
            .w_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .when(light, |element| element.bg(gpui::white()))
            .child(
                div()
                    .text_color(theme.tokens.text_secondary)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(question.title.clone()),
            )
            .child(answers)
            .into_any_element()
    }

    fn render_resources(&self, theme: &Theme, locale: &str, cx: &App) -> AnyElement {
        let store = OnboardingStore::global(cx);
        let store = store.read(cx);
        let rules = store
            .onboarding(self.clan_id)
            .map(|onboarding| onboarding.rules.as_slice())
            .unwrap_or_default();
        let mut section = v_flex()
            .gap_2()
            .child(section_title(mezon_i18n::t(locale, "guide.resources")));
        if rules.is_empty() {
            return section
                .child(empty_card(theme, mezon_i18n::t(locale, "guide.noRules")))
                .into_any_element();
        }
        for rule in rules {
            let thumbnail = div()
                .size(px(72.))
                .flex_none()
                .rounded(px(8.))
                .overflow_hidden()
                .when(!rule.image_url.is_empty(), |element| {
                    element.child(
                        img(proxied(cx, &rule.image_url, 144, 144, "fill"))
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .into_any_element();
            section = section.child(guide_card(
                SharedString::from(format!("clan-guide-rule-{}", rule.id)),
                theme,
                Icon::new(IconName::RuleIcon)
                    .size(px(24.))
                    .text_color(theme.tokens.text_theme_primary)
                    .into_any_element(),
                rule.title.clone().into(),
                div()
                    .text_size(px(12.))
                    .child(rule.content.clone())
                    .into_any_element(),
                Some(thumbnail),
            ));
        }
        section.into_any_element()
    }

    fn render_missions(&self, theme: &Theme, locale: &str, cx: &App) -> AnyElement {
        let store = OnboardingStore::global(cx);
        let store = store.read(cx);
        let missions = store
            .onboarding(self.clan_id)
            .map(|onboarding| onboarding.missions.as_slice())
            .unwrap_or_default();
        let mut section = v_flex()
            .gap_2()
            .child(section_title(mezon_i18n::t(locale, "guide.missions")));
        if missions.is_empty() {
            return section
                .child(empty_card(theme, mezon_i18n::t(locale, "guide.noMissions")))
                .into_any_element();
        }
        let clan_id = self.clan_id;
        let done = store.mission_done(clan_id);
        let channels = ChannelList::global(cx);
        let channels = channels.read(cx);
        for (index, mission) in missions.iter().enumerate() {
            let channel_id = ChannelId(mission.channel_id);
            let channel_label = channels
                .channel(clan_id, channel_id)
                .map(|channel| channel.name.clone())
                .unwrap_or_default();
            let description = h_flex()
                .gap_1()
                .child(mission_summary(locale, mission.task_type))
                .when(!channel_label.is_empty(), |element| {
                    element.child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_secondary)
                            .child(format!("#{channel_label}")),
                    )
                })
                .into_any_element();
            let tick = (done > index).then(|| {
                div()
                    .size(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Tick)
                            .size(px(24.))
                            .text_color(rgb(TICK_COLOR)),
                    )
                    .into_any_element()
            });
            let task_type = mission.task_type;
            section = section.child(
                guide_card(
                    SharedString::from(format!("clan-guide-mission-{}", mission.id)),
                    theme,
                    Icon::new(IconName::TargetIcon)
                        .size(px(24.))
                        .text_color(theme.tokens.text_theme_primary)
                        .into_any_element(),
                    mission.title.clone().into(),
                    description,
                    tick,
                )
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    start_mission(clan_id, channel_id, task_type, index, cx);
                }),
            );
        }
        section.into_any_element()
    }

    fn render_load_failure(&self, theme: &Theme, locale: &str) -> AnyElement {
        let clan_id = self.clan_id;
        h_flex()
            .gap_2()
            .h(px(80.))
            .p_4()
            .w_full()
            .items_center()
            .justify_between()
            .rounded(px(8.))
            .bg(theme.tokens.bg_active_member_channel)
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(mezon_i18n::t(locale, "guide.loadFailed")),
            )
            .child(
                div()
                    .id("clan-guide-retry")
                    .h(px(36.))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.tokens.border_primary)
                    .bg(theme.tokens.bg_tertiary)
                    .text_color(theme.tokens.text_theme_primary)
                    .cursor_pointer()
                    .hover(|style| {
                        style
                            .bg(theme.tokens.bg_secondary_button_hover)
                            .text_color(theme.tokens.text_secondary)
                    })
                    .child(mezon_i18n::t(locale, "guide.retry"))
                    .on_click(move |_, _, cx| {
                        OnboardingStore::global(cx)
                            .update(cx, |store, cx| store.reload(clan_id, cx));
                    }),
            )
            .into_any_element()
    }

    fn render_about(theme: &Theme, locale: &str) -> AnyElement {
        v_flex()
            .mt_8()
            .gap_2()
            .h(px(80.))
            .p_4()
            .w(px(300.))
            .flex_none()
            .justify_between()
            .rounded(px(8.))
            .bg(theme.tokens.bg_active_member_channel)
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(mezon_i18n::t(locale, "guide.about")),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .child(mezon_i18n::t(locale, "guide.membersOnline")),
            )
            .into_any_element()
    }
}

fn section_title(title: &'static str) -> AnyElement {
    div()
        .p_2()
        .text_size(px(20.))
        .font_weight(FontWeight::BOLD)
        .child(title)
        .into_any_element()
}

fn empty_card(theme: &Theme, message: &'static str) -> AnyElement {
    h_flex()
        .gap_2()
        .h(px(80.))
        .p_4()
        .w_full()
        .text_size(px(18.))
        .items_center()
        .font_weight(FontWeight::SEMIBOLD)
        .justify_between()
        .rounded(px(8.))
        .bg(theme.tokens.bg_active_member_channel)
        .child(message)
        .into_any_element()
}

fn guide_card(
    id: impl Into<ElementId>,
    theme: &Theme,
    icon: AnyElement,
    title: SharedString,
    description: AnyElement,
    action: Option<AnyElement>,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_start()
        .gap_2()
        .p_4()
        .rounded(px(8.))
        .border_1()
        .border_color(theme.tokens.border_primary)
        .bg(theme.tokens.bg_active_member_channel)
        .text_color(theme.tokens.text_theme_primary)
        .hover(|style| {
            style
                .bg(theme.tokens.bg_item_hover)
                .text_color(theme.tokens.text_secondary)
        })
        .overflow_x_hidden()
        .child(
            div()
                .size(px(48.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .child(icon),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .justify_center()
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.tokens.text_secondary)
                        .truncate()
                        .child(title),
                )
                .child(description),
        )
        .when_some(action, |card, action| {
            card.child(div().flex().items_center().flex_none().child(action))
        })
}

fn mission_summary(locale: &str, task_type: i32) -> &'static str {
    let key = match task_type {
        MISSION_VISIT => "guide.missionTitles.visit",
        MISSION_DO_SOMETHING => "guide.missionTitles.doSomething",
        _ => "guide.missionTitles.sendMessage",
    };
    mezon_i18n::t(locale, key)
}

fn start_mission(
    clan_id: ClanId,
    channel_id: ChannelId,
    task_type: i32,
    index: usize,
    cx: &mut App,
) {
    let store = OnboardingStore::global(cx);
    if !store.read(cx).can_start_mission(clan_id, index) {
        return;
    }
    match task_type {
        MISSION_SEND_MESSAGE => open_channel(clan_id, channel_id, cx),
        MISSION_VISIT => {
            open_channel(clan_id, channel_id, cx);
            store.update(cx, |store, cx| store.complete_mission(clan_id, cx));
        }
        MISSION_DO_SOMETHING => {
            store.update(cx, |store, cx| store.complete_mission(clan_id, cx));
        }
        _ => {}
    }
}

fn open_channel(clan_id: ClanId, channel_id: ChannelId, cx: &mut App) {
    if channel_id.is_zero() {
        return;
    }
    ChannelList::global(cx).update(cx, |list, cx| list.select_channel(channel_id, cx));
    MessagesStore::global(cx).update(cx, |store, cx| {
        store.open_channel_in_clan(clan_id, channel_id, cx)
    });
    crate::router::navigate(
        cx,
        crate::router::Route::Channel {
            clan_id,
            channel_id,
        },
    );
}

impl Render for ClanGuidePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let theme = cx.theme();
        let body = div()
            .id("clan-guide-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .items_center()
            .p_8()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .text_color(theme.tokens.text_theme_primary)
            .child(self.render_banner(theme, cx))
            .child(self.render_header(theme, &locale, cx))
            .child(
                div().pt_8().w_full().flex_none().child(
                    h_flex()
                        .w_full()
                        .items_start()
                        .gap_6()
                        .child(v_flex().flex_1().min_w_0().gap_2().map(|column| {
                            match self.content_state(cx) {
                                ContentState::Failed => {
                                    column.child(self.render_load_failure(theme, &locale))
                                }
                                ContentState::Loading => column,
                                ContentState::Ready => column
                                    .child(self.render_questions(theme, &locale, cx))
                                    .child(self.render_resources(theme, &locale, cx))
                                    .child(self.render_missions(theme, &locale, cx)),
                            }
                        }))
                        .child(Self::render_about(theme, &locale)),
                ),
            )
            .into_any_element();
        management_page(
            mezon_i18n::t(&locale, "channelTopbar.pageTitle.guideClan"),
            body,
            theme,
        )
    }
}
