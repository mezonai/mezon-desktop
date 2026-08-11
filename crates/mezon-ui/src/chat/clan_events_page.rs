use chrono::{DateTime, Local};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, FontWeight, Render, Subscription,
    Window, div, img, prelude::*, px,
};
use mezon_store::{
    BadgeService, ChannelList, ChannelType, ClanEventItem, ClanId, ClanMembersStore, EventsStore,
    Settings,
};

use crate::app::shell::Shell;
use crate::components::primitives::{Avatar, Button, ButtonVariants, Icon, IconName};
use crate::theme::ActiveTheme;

pub struct ClanEventsModal {
    clan_id: ClanId,
    settings: Entity<Settings>,
    focus_handle: FocusHandle,
    _events_subscription: Subscription,
    _members_subscription: Subscription,
    _channels_subscription: Subscription,
}

impl Focusable for ClanEventsModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ClanEventsModal {
    pub fn new(
        clan_id: ClanId,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let events_subscription = cx.observe(&EventsStore::global(cx), |_, _, cx| cx.notify());
        let members_subscription =
            cx.observe(&ClanMembersStore::global(cx), |_, _, cx| cx.notify());
        let channels_subscription = cx.observe(&ChannelList::global(cx), |_, _, cx| cx.notify());
        EventsStore::global(cx).update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
        ClanMembersStore::global(cx).update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
        ChannelList::global(cx).update(cx, |store, cx| store.load_for_clan(clan_id, cx));
        Self {
            clan_id,
            settings,
            focus_handle,
            _events_subscription: events_subscription,
            _members_subscription: members_subscription,
            _channels_subscription: channels_subscription,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn event_card(&self, event: &ClanEventItem, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let tr = |key| mezon_i18n::t(&locale, key);
        let channels_entity = ChannelList::global(cx);
        let channels = channels_entity.read(cx);
        let channel = event
            .channel_id
            .and_then(|id| channels.channel(self.clan_id, id));
        let voice_name = event
            .channel_voice_id
            .and_then(|id| channels.channel_display_name(self.clan_id, id));
        let is_location = !event.address.is_empty();
        let location = if event.is_private {
            tr("eventCreator.eventDetail.privateRoom").to_string()
        } else if is_location {
            event.address.clone()
        } else {
            voice_name.unwrap_or_else(|| tr("eventCreator.tabs.location").to_string())
        };
        let is_channel_event = event.channel_id.is_some();
        let creator = ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, event.creator_id)
            .cloned();
        let creator_name = creator
            .as_ref()
            .map(|member| member.name().to_string())
            .unwrap_or_else(|| tr("eventCreator.eventDetail.createdBy").to_string());
        let creator_avatar = creator
            .as_ref()
            .map(|member| member.avatar().to_string())
            .unwrap_or_default();
        let interested = event.user_ids.iter().filter(|id| id.0 != 0).count();
        let current_user = BadgeService::global(cx).read(cx).current_user_id(cx);
        let user_is_interested =
            current_user.is_some_and(|user_id| event.user_ids.contains(&user_id));
        let now = chrono::Utc::now().timestamp().max(0) as u32;
        let end = if event.end_time_seconds == 0 {
            event.start_time_seconds.saturating_add(2 * 60 * 60)
        } else {
            event.end_time_seconds
        };
        let ongoing = event.start_time_seconds <= now && now <= end;
        let scheduled_date = DateTime::from_timestamp(event.start_time_seconds as i64, 0)
            .map(|date| {
                date.with_timezone(&Local)
                    .format("%a, %b %-d - %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| tr("eventCreator.actions.noEvent").to_string());
        let audience_channel = channel.map(|channel| {
            (
                if channel.channel_type == ChannelType::Thread {
                    "thread"
                } else {
                    "channel"
                },
                channel.name.clone(),
            )
        });
        let starts_soon = event.start_time_seconds > now
            && event.start_time_seconds.saturating_sub(now) <= 10 * 60;
        let minutes_until = event
            .start_time_seconds
            .saturating_sub(now)
            .div_ceil(60)
            .max(1);
        let date = if ongoing {
            tr("eventMenu.countdown.joinNow").to_string()
        } else if starts_soon {
            tr("eventMenu.countdown.joinIn_other").replace("{{count}}", &minutes_until.to_string())
        } else {
            scheduled_date
        };
        let status_color = if ongoing {
            theme.status_online
        } else if starts_soon {
            gpui::rgb(0xa855f7)
        } else {
            theme.text_secondary
        };

        let heading = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::IconEvents)
                            .size(px(25.))
                            .text_color(status_color),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(status_color)
                            .text_size(px(15.))
                            .child(date),
                    )
                    .child(
                        div()
                            .px_1()
                            .py(px(2.))
                            .rounded(px(2.))
                            .bg(if event.is_private {
                                gpui::rgb(0xef4444)
                            } else if is_channel_event {
                                gpui::rgb(0xf97316)
                            } else {
                                gpui::rgb(0x3b82f6)
                            })
                            .text_color(gpui::white())
                            .text_size(px(14.))
                            .child(if event.is_private {
                                tr("eventCreator.eventDetail.privateEvent")
                            } else if is_channel_event {
                                tr("eventCreator.eventDetail.channelEvent")
                            } else {
                                tr("eventCreator.eventDetail.clanEvent")
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        Avatar::new()
                            .name(creator_name)
                            .src(creator_avatar)
                            .size_px(px(28.)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_1()
                            .rounded_full()
                            .bg(theme.tokens.bg_secondary)
                            .child(interested.to_string())
                            .child(
                                Icon::new(IconName::MemberList)
                                    .size_4()
                                    .text_color(theme.text_primary),
                            ),
                    ),
            );

        let mut details = div().flex().justify_between().gap_4().mt_4().child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(16.))
                        .text_color(theme.text_secondary)
                        .truncate()
                        .child(event.title.clone()),
                )
                .child(
                    div()
                        .mt_1()
                        .text_color(theme.text_secondary)
                        .text_size(px(14.))
                        .line_clamp(4)
                        .text_ellipsis()
                        .child(event.description.clone()),
                ),
        );
        if !event.logo.is_empty() {
            let logo = crate::util::imgproxy::proxied(cx, &event.logo, 400, 220, "fit");
            details = details.child(
                img(logo)
                    .w(px(200.))
                    .h(px(110.))
                    .rounded(px(3.))
                    .object_fit(gpui::ObjectFit::Contain),
            );
        }

        let action = |icon, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_4()
                .py_2()
                .rounded_lg()
                .bg(theme.tokens.bg_secondary)
                .text_color(theme.text_secondary)
                .child(Icon::new(icon).size_4().text_color(theme.text_secondary))
                .child(div().text_color(theme.text_secondary).child(label))
        };
        let interest_action = action(
            if user_is_interested {
                IconName::MuteBell
            } else {
                IconName::Bell
            },
            tr(if user_is_interested {
                "eventMenu.dashboard.UnInterested"
            } else {
                "eventMenu.dashboard.Interested"
            }),
        );

        div()
            .w_full()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.bg_theme_direct_message)
            .overflow_hidden()
            .child(div().p_4().child(heading).child(details))
            .child(
                div()
                    .border_t_1()
                    .border_color(theme.border)
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .text_size(px(14.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .min_w_0()
                                    .text_color(theme.text_secondary)
                                    .child(if is_location && !event.is_private {
                                        img(IconName::Location.path())
                                            .size(px(20.))
                                            .into_any_element()
                                    } else {
                                        Icon::new(IconName::Speaker)
                                            .size(px(20.))
                                            .text_color(theme.text_secondary)
                                            .into_any_element()
                                    })
                                    .child(
                                        div()
                                            .truncate()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(location),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div().px_2().text_color(theme.text_secondary).child(
                                            Icon::new(IconName::ThreeDot)
                                                .size(px(20.))
                                                .text_color(theme.text_secondary),
                                        ),
                                    )
                                    .when(!is_location, |actions| {
                                        actions.child(action(
                                            IconName::IconShareEventVoice,
                                            tr("eventCreator.eventDetail.share"),
                                        ))
                                    })
                                    .child(if ongoing {
                                        div()
                                            .px_4()
                                            .py_2()
                                            .rounded_lg()
                                            .bg(theme.bg_tertiary)
                                            .text_color(theme.text_secondary)
                                            .child(
                                                div()
                                                    .text_color(theme.text_secondary)
                                                    .child(tr("eventMenu.dashboard.endEvent")),
                                            )
                                    } else {
                                        interest_action
                                    }),
                            ),
                    )
                    .child(if event.is_private {
                        div()
                            .mt_3()
                            .flex()
                            .justify_between()
                            .items_start()
                            .gap_3()
                            .text_color(theme.text_secondary)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .child(tr("eventCreator.eventDetail.onlyInvitedMembers")),
                            )
                            .when(!event.external_link.is_empty(), |row| {
                                row.child(
                                    div()
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .text_color(theme.text_link)
                                        .gap_3()
                                        .child(
                                            div()
                                                .id("event-open-link")
                                                .whitespace_nowrap()
                                                .child(tr("eventCreator.eventDetail.openLink")),
                                        )
                                        .child(
                                            div()
                                                .id("event-invite")
                                                .whitespace_nowrap()
                                                .child(tr("eventCreator.eventDetail.invite")),
                                        )
                                        .child(
                                            div()
                                                .id("event-copy-link")
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .whitespace_nowrap()
                                                .child(tr("eventCreator.eventDetail.copyLink"))
                                                .child(
                                                    Icon::new(IconName::CopyIcon)
                                                        .size(px(17.))
                                                        .text_color(theme.text_link),
                                                ),
                                        ),
                                )
                            })
                    } else if let Some((kind, channel_name)) = audience_channel {
                        div()
                            .mt_3()
                            .flex()
                            .items_center()
                            .text_color(theme.text_secondary)
                            .child(format!(
                                "{} {}",
                                tr("eventCreator.eventDetail.audienceConsists"),
                                tr(if kind == "thread" {
                                    "eventCreator.eventDetail.thread"
                                } else {
                                    "eventCreator.eventDetail.channel"
                                })
                            ))
                            .child(
                                div()
                                    .text_color(theme.text_primary)
                                    .font_weight(FontWeight::BOLD)
                                    .child(channel_name),
                            )
                    } else {
                        div()
                            .mt_3()
                            .text_color(theme.text_secondary)
                            .child(tr("eventMenu.dashboard.noti"))
                    }),
            )
            .into_any_element()
    }
}

impl Render for ClanEventsModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let tr = |key| mezon_i18n::t(&locale, key);
        let current_user = BadgeService::global(cx).read(cx).current_user_id(cx);
        let events =
            EventsStore::global(cx)
                .read(cx)
                .visible_events(self.clan_id, current_user, cx);
        let mut list = div().w_full().flex().flex_col().gap_4();
        for event in &events {
            list = list.child(self.event_card(event, cx));
        }
        if events.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .py_8()
                    .text_center()
                    .child(
                        div()
                            .relative()
                            .w(px(82.))
                            .h(px(82.))
                            .text_color(theme.text_muted)
                            .child(
                                div().absolute().left(px(10.)).top(px(10.)).child(
                                    Icon::new(IconName::IconEvents)
                                        .size(px(64.))
                                        .text_color(theme.text_muted),
                                ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right(px(1.))
                                    .top(px(8.))
                                    .text_size(px(13.))
                                    .text_color(gpui::rgb(0x22d3ee))
                                    .child("✦"),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(0.))
                                    .bottom(px(7.))
                                    .text_size(px(18.))
                                    .text_color(gpui::rgb(0xfacc15))
                                    .child("✦"),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right(px(13.))
                                    .top(px(7.))
                                    .size(px(6.))
                                    .rounded_full()
                                    .bg(gpui::rgb(0x22d3ee)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right(px(16.))
                                    .bottom(px(4.))
                                    .size(px(4.))
                                    .rounded_full()
                                    .bg(gpui::rgb(0x22d3ee)),
                            ),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_size(px(17.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary)
                            .child(tr("eventCreator.emptyState.title")),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme.text_secondary)
                            .child(format!(
                                "{} {}.",
                                tr("eventCreator.emptyState.description1"),
                                tr("eventCreator.emptyState.clan")
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme.text_secondary)
                            .child(tr("eventCreator.emptyState.description2")),
                    ),
            );
        }
        let modal_height = if events.is_empty() {
            px(370.)
        } else {
            px((100 + events.len().min(2) * 310).min(600) as f32)
        };
        let event_count_label = if events.len() == 1 {
            tr("eventCreator.actions.event_one").to_string()
        } else if events.is_empty() {
            tr("eventCreator.actions.noEvent").to_string()
        } else {
            tr("eventCreator.actions.event_other").replace("{{count}}", &events.len().to_string())
        };

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                this.close(cx);
            }))
            .w(px(600.))
            .h(modal_height)
            .flex()
            .flex_col()
            .rounded_lg()
            .overflow_hidden()
            .bg(theme.bg_primary)
            .text_color(theme.text_secondary)
            .child(
                div()
                    .h(px(72.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px_5()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_size(px(15.))
                            .child(
                                Icon::new(IconName::IconEvents)
                                    .size(px(25.))
                                    .text_color(theme.text_secondary),
                            )
                            .child(div().font_weight(FontWeight::BOLD).child(event_count_label))
                            .child(div().h(px(28.)).w(px(2.)).bg(theme.border))
                            .child(
                                Button::new("create-event")
                                    .label(tr("eventCreator.actions.create"))
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        super::create_event_modal::open_create_event_modal(
                                            this.clan_id,
                                            this.settings.clone(),
                                            window,
                                            cx,
                                        );
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("close-events-modal")
                            .group("close-events-modal")
                            .cursor_pointer()
                            .p_2()
                            .text_color(theme.text_secondary)
                            .hover(|style| style.text_color(theme.text_primary))
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
                            .child(
                                Icon::new(IconName::CloseButton)
                                    .size(px(20.))
                                    .text_color(theme.text_secondary)
                                    .group_hover("close-events-modal", |style| {
                                        style.text_color(theme.text_primary)
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("clan-events-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .py_5()
                    .child(list),
            )
    }
}

pub fn open_clan_events_modal(
    clan_id: ClanId,
    settings: Entity<Settings>,
    window: &mut Window,
    cx: &mut App,
) {
    let modal = cx.new(|cx| ClanEventsModal::new(clan_id, settings, window, cx));
    Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
}
