use chrono::{DateTime, Local};
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    FontWeight, Pixels, Point, Render, Subscription, Task, Window, div, img, point, prelude::*, px,
};
use mezon_store::{
    AppConfig, BadgeService, ChannelList, ChannelType, ClanEventItem, ClanId, ClanList,
    ClanMembersStore, EventsStore, PermissionStore, PlatformStore, Settings, UserId,
};
use std::collections::HashSet;
use std::time::Duration;

use crate::app::shell::Shell;
use crate::chat::media_channel::media_image::render_media_image_rounded_top;
use crate::clan::invite_people_modal::open_external_event_invite_modal;
use crate::components::primitives::{Avatar, Button, ButtonVariants, Icon, IconName};
use crate::components::primitives::{ContextMenu, context_menu_at};
use crate::image_cache::LruImageCache;
use crate::theme::ActiveTheme;

const EVENT_IMAGE_CACHE_CAPACITY: usize = 8;
const EVENT_IMAGE_CACHE_BYTES: u64 = 8 * 1024 * 1024;
const EVENT_IMAGE_CACHE_ENTRY_BYTES: u64 = 2 * 1024 * 1024;

pub struct ClanEventsModal {
    clan_id: ClanId,
    settings: Entity<Settings>,
    focus_handle: FocusHandle,
    _events_subscription: Subscription,
    _members_subscription: Subscription,
    _channels_subscription: Subscription,
    selected_event_id: Option<i64>,
    detail_tab: EventDetailTab,
    menu_event_id: Option<i64>,
    menu_position: Option<Point<Pixels>>,
    pending_interest_event_ids: HashSet<i64>,
    copied_external_event_id: Option<i64>,
    copy_reset_task: Option<Task<()>>,
    image_cache: Entity<LruImageCache>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventDetailTab {
    Info,
    Interested,
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
        let image_cache = cx.new(|cx| {
            LruImageCache::gallery_preview(
                "clan-events-modal",
                EVENT_IMAGE_CACHE_CAPACITY,
                EVENT_IMAGE_CACHE_BYTES,
                EVENT_IMAGE_CACHE_ENTRY_BYTES,
                cx,
            )
        });
        Self {
            clan_id,
            settings,
            focus_handle,
            _events_subscription: events_subscription,
            _members_subscription: members_subscription,
            _channels_subscription: channels_subscription,
            selected_event_id: None,
            detail_tab: EventDetailTab::Info,
            menu_event_id: None,
            menu_position: None,
            pending_interest_event_ids: HashSet::new(),
            copied_external_event_id: None,
            copy_reset_task: None,
            image_cache,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn reset_external_copy_after_leave(&mut self, event_id: i64, cx: &mut Context<Self>) {
        if self.copied_external_event_id != Some(event_id) {
            return;
        }
        self.copy_reset_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            this.update(cx, |this, cx| {
                if this.copied_external_event_id == Some(event_id) {
                    this.copied_external_event_id = None;
                    cx.notify();
                }
                this.copy_reset_task = None;
            })
            .ok();
        }));
    }

    fn update_interest(
        &mut self,
        event_id: i64,
        user_id: UserId,
        interested: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.pending_interest_event_ids.insert(event_id) {
            return;
        }
        cx.notify();
        let task = EventsStore::global(cx).update(cx, |store, cx| {
            store.set_user_interested(self.clan_id, event_id, user_id, interested, cx)
        });
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                tracing::warn!(%error, event_id, "failed to update event interest");
            }
            this.update(cx, |this, cx| {
                this.pending_interest_event_ids.remove(&event_id);
                cx.notify();
            })
            .ok();
        })
        .detach();
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
            voice_name
                .clone()
                .unwrap_or_else(|| tr("eventCreator.tabs.location").to_string())
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
        let interest_pending = self.pending_interest_event_ids.contains(&event.id);
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
                            .bg(theme.surfaces.secondary)
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
                        .id(("event-id", event.id as usize))
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(16.))
                        .text_color(theme.text_secondary)
                        .truncate()
                        .cursor_pointer()
                        .hover(|style| style.underline())
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

        let action = |group: &'static str, icon, label: &'static str| {
            div()
                .group(group)
                .flex()
                .items_center()
                .gap_2()
                .px_4()
                .py_2()
                .rounded_lg()
                .bg(theme.surfaces.secondary)
                .text_color(theme.text_secondary)
                .hover(|style| style.bg(theme.bg_hover))
                .child(
                    Icon::new(icon)
                        .size_4()
                        .text_color(theme.text_secondary)
                        .group_hover(group, |style| style.text_color(theme.text_primary)),
                )
                .child(
                    div()
                        .text_color(theme.text_secondary)
                        .group_hover(group, |style| style.text_color(theme.text_primary))
                        .child(label),
                )
        };
        let interest_action = action(
            "event-interest-action",
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
        )
        .id(("event-interest", event.id as usize))
        .when(interest_pending, |action| {
            action.cursor_not_allowed().opacity(0.5)
        })
        .when(!interest_pending, |action| action.cursor_pointer());

        let permissions = PermissionStore::try_global(cx)
            .map(|store| store.read(cx).clan_settings_permissions(self.clan_id, cx));
        let can_end_event = permissions.is_some_and(|permissions| permissions.is_clan_owner);
        let share_link = self.event_link(event, cx);
        let external_link_copied = self.copied_external_event_id == Some(event.id);
        let share_channel_name = voice_name.unwrap_or_else(|| location.clone());
        let event_for_end = event.clone();

        let event_id = event.id;
        div()
            .id(("event-card", event_id as usize))
            .relative()
            .w_full()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.surfaces.direct_message)
            .child(
                div()
                    .id(("event-summary", event_id as usize))
                    .p_4()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_event_id = Some(event_id);
                        this.detail_tab = EventDetailTab::Info;
                        this.menu_event_id = None;
                        this.menu_position = None;
                        cx.notify();
                    }))
                    .child(heading)
                    .child(details),
            )
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
                                        div()
                                            .id(("event-menu-button", event_id as usize))
                                            .group("event-three-dot")
                                            .px_2()
                                            .text_color(theme.text_secondary)
                                            .cursor_pointer()
                                            .on_click(cx.listener(
                                                move |this, event: &ClickEvent, _, cx| {
                                                    cx.stop_propagation();
                                                    let opening =
                                                        this.menu_event_id != Some(event_id);
                                                    this.menu_event_id =
                                                        opening.then_some(event_id);
                                                    this.menu_position = opening.then(|| {
                                                        let position = event.position();
                                                        point(position.x + px(10.), position.y)
                                                    });
                                                    cx.notify();
                                                },
                                            ))
                                            .child(
                                                Icon::new(IconName::ThreeDot)
                                                    .size(px(20.))
                                                    .text_color(theme.text_secondary)
                                                    .group_hover("event-three-dot", |style| {
                                                        style.text_color(theme.text_primary)
                                                    }),
                                            ),
                                    )
                                    .when(!is_location && !share_link.is_empty(), |actions| {
                                        actions.child(
                                            action(
                                                "event-share-action",
                                                IconName::IconShareEventVoice,
                                                tr("eventCreator.eventDetail.share"),
                                            )
                                            .id(("share-event", event_id as usize))
                                            .cursor_pointer()
                                            .on_click(
                                                cx.listener({
                                                    let share_link = share_link.clone();
                                                    let share_channel_name =
                                                        share_channel_name.clone();
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        open_share_event_modal(
                                                            share_channel_name.clone(),
                                                            share_link.clone(),
                                                            this.settings.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    }
                                                }),
                                            ),
                                        )
                                    })
                                    .when(ongoing && can_end_event, |actions| {
                                        actions.child(
                                            div()
                                                .id(("end-event", event_id as usize))
                                                .group("event-end-action")
                                                .px_4()
                                                .py_2()
                                                .rounded_lg()
                                                .bg(theme.bg_tertiary)
                                                .text_color(theme.text_secondary)
                                                .cursor_pointer()
                                                .hover(|style| style.bg(theme.bg_hover))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        open_cancel_event_confirm(
                                                            this.clan_id,
                                                            event_for_end.clone(),
                                                            this.settings.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .text_color(theme.text_secondary)
                                                        .group_hover("event-end-action", |style| {
                                                            style.text_color(theme.text_primary)
                                                        })
                                                        .child(tr("eventMenu.dashboard.endEvent")),
                                                ),
                                        )
                                    })
                                    .when(!ongoing, |actions| {
                                        actions.child(interest_action.when(
                                            !interest_pending,
                                            |action| {
                                                action.on_click(cx.listener(
                                                    move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        let Some(user_id) = current_user else {
                                                            return;
                                                        };
                                                        this.update_interest(
                                                            event_id,
                                                            user_id,
                                                            !user_is_interested,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                            },
                                        ))
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
                                let open_link = share_link.clone();
                                let invite_link = share_link.clone();
                                let copy_link = share_link.clone();
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
                                                .cursor_pointer()
                                                .hover(|style| {
                                                    style.underline().text_color(theme.text_primary)
                                                })
                                                .on_click(move |_, _, cx| {
                                                    cx.stop_propagation();
                                                    if let Some(platform) =
                                                        PlatformStore::try_global(cx)
                                                    {
                                                        let _ = platform
                                                            .read(cx)
                                                            .open_url_external(&open_link);
                                                    }
                                                })
                                                .child(tr("eventCreator.eventDetail.openLink")),
                                        )
                                        .child(
                                            div()
                                                .id("event-invite")
                                                .whitespace_nowrap()
                                                .cursor_pointer()
                                                .hover(|style| {
                                                    style.underline().text_color(theme.text_primary)
                                                })
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        let clan = ClanList::global(cx)
                                                            .read(cx)
                                                            .clan(this.clan_id)
                                                            .cloned();
                                                        let clan_name = clan
                                                            .as_ref()
                                                            .map(|clan| clan.name.clone())
                                                            .unwrap_or_default();
                                                        let clan_avatar = clan
                                                            .and_then(|clan| clan.avatar_url)
                                                            .unwrap_or_default();
                                                        let locale =
                                                            this.settings.read(cx).language.clone();
                                                        open_external_event_invite_modal(
                                                            this.clan_id,
                                                            clan_name,
                                                            clan_avatar,
                                                            locale,
                                                            invite_link.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                                .child(tr("eventCreator.eventDetail.invite")),
                                        )
                                        .child(
                                            div()
                                                .id("event-copy-link")
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .whitespace_nowrap()
                                                .cursor_pointer()
                                                .hover(|style| {
                                                    style.underline().text_color(theme.text_primary)
                                                })
                                                .on_hover(cx.listener(
                                                    move |this, hovered: &bool, _, cx| {
                                                        if !hovered {
                                                            this.reset_external_copy_after_leave(
                                                                event_id, cx,
                                                            );
                                                        }
                                                    },
                                                ))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    if this.copied_external_event_id
                                                        == Some(event_id)
                                                    {
                                                        return;
                                                    }
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(
                                                            copy_link.clone(),
                                                        ),
                                                    );
                                                    this.copied_external_event_id = Some(event_id);
                                                    this.copy_reset_task = None;
                                                    cx.notify();
                                                }))
                                                .child(tr("eventCreator.eventDetail.copyLink"))
                                                .child(
                                                    Icon::new(if external_link_copied {
                                                        IconName::Tick
                                                    } else {
                                                        IconName::CopyIcon
                                                    })
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

    fn event_link(&self, event: &ClanEventItem, cx: &App) -> String {
        if event.is_private && !event.external_link.is_empty() {
            format!(
                "{}{}",
                AppConfig::global(cx).redirect_uri.trim_end_matches('/'),
                event.external_link
            )
        } else {
            event
                .channel_voice_id
                .or(event.channel_id)
                .map_or_else(String::new, |channel_id| {
                    AppConfig::global(cx)
                        .channel_link(&self.clan_id.0.to_string(), &channel_id.0.to_string())
                })
        }
    }

    fn event_context_menu(
        &self,
        event: &ClanEventItem,
        position: Point<Pixels>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let locale = self.settings.read(cx).language.clone();
        let current_user = BadgeService::global(cx).read(cx).current_user_id(cx);
        let can_modify = current_user == Some(event.creator_id)
            || PermissionStore::try_global(cx).is_some_and(|store| {
                let permissions = store.read(cx).clan_settings_permissions(self.clan_id, cx);
                permissions.is_clan_owner
                    || permissions.has_manage_clan
                    || permissions.has_administrator
            });
        let event_link = self.event_link(event, cx);
        let weak = cx.weak_entity();
        let mut menu = ContextMenu::new().min_width(px(200.));

        if can_modify {
            let edit_event = event.clone();
            let weak_edit = weak.clone();
            menu = menu.item(
                mezon_i18n::t(&locale, "eventCreator.actions.editEvent"),
                move |window, cx| {
                    weak_edit
                        .update(cx, |this, cx| {
                            this.menu_event_id = None;
                            this.menu_position = None;
                            super::create_event_modal::open_edit_event_modal(
                                this.clan_id,
                                this.settings.clone(),
                                edit_event.clone(),
                                window,
                                cx,
                            );
                        })
                        .ok();
                },
            );

            let cancel_event = event.clone();
            let weak_cancel = weak.clone();
            menu = menu.danger_item(
                mezon_i18n::t(&locale, "eventCreator.actions.cancelEvent"),
                move |window, cx| {
                    weak_cancel
                        .update(cx, |this, cx| {
                            this.menu_event_id = None;
                            this.menu_position = None;
                            open_cancel_event_confirm(
                                this.clan_id,
                                cancel_event.clone(),
                                this.settings.clone(),
                                window,
                                cx,
                            );
                        })
                        .ok();
                },
            );
        }

        if !event_link.is_empty() {
            let weak_copy = weak.clone();
            menu = menu.item(
                mezon_i18n::t(&locale, "eventCreator.actions.copyEventLink"),
                move |_window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(event_link.clone()));
                    weak_copy
                        .update(cx, |this, cx| {
                            this.menu_event_id = None;
                            this.menu_position = None;
                            cx.notify();
                        })
                        .ok();
                },
            );
        }

        menu = menu.on_dismiss(move |_window, cx| {
            weak.update(cx, |this, cx| {
                this.menu_event_id = None;
                this.menu_position = None;
                cx.notify();
            })
            .ok();
        });

        context_menu_at(position, menu).into_any_element()
    }

    fn event_detail(&self, event: &ClanEventItem, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let tr = |key| mezon_i18n::t(&locale, key);
        let interested_ids: Vec<_> = event
            .user_ids
            .iter()
            .filter(|id| id.0 != 0)
            .copied()
            .collect();
        let interested_count = interested_ids.len();
        let creator = ClanMembersStore::global(cx)
            .read(cx)
            .member(self.clan_id, event.creator_id)
            .cloned();
        let creator_name = creator
            .as_ref()
            .map(|member| member.name())
            .unwrap_or_default();
        let creator_avatar = creator
            .as_ref()
            .map(|member| member.avatar())
            .unwrap_or_default();
        let clan = ClanList::global(cx).read(cx).clan(self.clan_id).cloned();
        let clan_name = clan
            .as_ref()
            .map(|clan| clan.name.as_str())
            .unwrap_or_default();
        let clan_avatar = clan
            .as_ref()
            .and_then(|clan| clan.avatar_url.as_deref())
            .unwrap_or_default();
        let channels = ChannelList::global(cx);
        let channels = channels.read(cx);
        let voice_name = event
            .channel_voice_id
            .and_then(|id| channels.channel_display_name(self.clan_id, id));
        let location = if !event.address.is_empty() {
            event.address.clone()
        } else if event.is_private {
            tr("eventCreator.eventDetail.privateRoom").to_string()
        } else {
            voice_name.unwrap_or_else(|| tr("eventCreator.tabs.location").to_string())
        };
        let date = DateTime::from_timestamp(event.start_time_seconds as i64, 0)
            .map(|date| {
                date.with_timezone(&Local)
                    .format("%a, %b %-d - %H:%M")
                    .to_string()
            })
            .unwrap_or_default();

        let tab = |id: &'static str, label: String, selected: bool, target: EventDetailTab| {
            div()
                .id(id)
                .pb_4()
                .cursor_pointer()
                .font_weight(FontWeight::BOLD)
                .text_color(if selected {
                    theme.text_primary
                } else {
                    theme.text_secondary
                })
                .when(selected, |tab| {
                    tab.border_b_1().border_color(theme.text_primary)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.detail_tab = target;
                    cx.notify();
                }))
                .child(label)
        };

        let body = if self.detail_tab == EventDetailTab::Info {
            div()
                .p_5()
                .flex()
                .flex_col()
                .gap_3()
                .text_color(theme.text_secondary)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            Icon::new(IconName::IconEvents)
                                .size(px(22.))
                                .text_color(theme.text_secondary),
                        )
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(date)),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_primary)
                        .child(event.title.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            Avatar::new()
                                .name(clan_name.to_string())
                                .src(clan_avatar.to_string())
                                .size_px(px(22.)),
                        )
                        .child(clan_name.to_string()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(if !event.address.is_empty() {
                            img(IconName::Location.path())
                                .size(px(20.))
                                .into_any_element()
                        } else {
                            Icon::new(IconName::Speaker)
                                .size(px(20.))
                                .text_color(theme.text_secondary)
                                .into_any_element()
                        })
                        .child(location),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            Icon::new(IconName::MemberList)
                                .size(px(20.))
                                .text_color(theme.text_secondary),
                        )
                        .child(if interested_count == 1 {
                            tr("eventCreator.eventDetail.personInterested")
                                .replace("{{count}}", "1")
                        } else {
                            tr("eventCreator.eventDetail.personInteresteds")
                                .replace("{{count}}", &interested_count.to_string())
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            Avatar::new()
                                .name(creator_name.to_string())
                                .src(creator_avatar.to_string())
                                .size_px(px(22.)),
                        )
                        .child(
                            tr("eventCreator.eventDetail.createdBy")
                                .replace("{{username}}", creator_name),
                        ),
                )
                .when(!event.description.is_empty(), |body| {
                    body.child(div().whitespace_normal().child(event.description.clone()))
                })
                .into_any_element()
        } else {
            let members = ClanMembersStore::global(cx);
            let members = members.read(cx);
            let mut list = div().p_4().flex().flex_col().gap_1();
            for user_id in &interested_ids {
                if let Some(member) = members.member(self.clan_id, *user_id) {
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .p_2()
                            .rounded_md()
                            .bg(theme.surfaces.secondary)
                            .child(
                                Avatar::new()
                                    .name(member.name().to_string())
                                    .src(member.avatar().to_string())
                                    .size_px(px(28.)),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text_primary)
                                    .child(member.name().to_string()),
                            ),
                    );
                }
            }
            if interested_ids.is_empty() {
                list = list.child(
                    div()
                        .h(px(180.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme.text_secondary)
                        .child(tr("eventCreator.eventDetail.noOneInterested")),
                );
            }
            list.into_any_element()
        };

        div()
            .w(px(600.))
            .max_h(px(600.))
            .image_cache(self.image_cache.clone())
            .rounded_lg()
            .overflow_hidden()
            .bg(theme.bg_primary)
            .when(!event.logo.is_empty(), |detail| {
                let banner = crate::util::imgproxy::proxied(cx, &event.logo, 1200, 352, "fill");
                detail.child(
                    div()
                        .w_full()
                        .h(px(176.))
                        .rounded_t(px(8.))
                        .overflow_hidden()
                        .child(render_media_image_rounded_top(
                            theme.bg_tertiary,
                            theme.text_muted,
                            banner.into(),
                            px(600.),
                            px(176.),
                            px(8.),
                        )),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .pt_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .gap_6()
                            .child(tab(
                                "event-detail-info",
                                tr("eventCreator.eventDetail.eventInfo").to_string(),
                                self.detail_tab == EventDetailTab::Info,
                                EventDetailTab::Info,
                            ))
                            .child(tab(
                                "event-detail-interested",
                                tr("eventCreator.eventDetail.interested")
                                    .replace("{{count}}", &interested_count.to_string()),
                                self.detail_tab == EventDetailTab::Interested,
                                EventDetailTab::Interested,
                            )),
                    )
                    .child(
                        div()
                            .id("close-event-detail")
                            .pb_4()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected_event_id = None;
                                cx.notify();
                            }))
                            .child(
                                Icon::new(IconName::CloseButton)
                                    .size(px(18.))
                                    .text_color(theme.text_primary),
                            ),
                    ),
            )
            .child(
                div()
                    .id("event-detail-scroll")
                    .max_h(px(330.))
                    .overflow_y_scroll()
                    .child(body),
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
        if let Some(event) = self
            .selected_event_id
            .and_then(|event_id| events.iter().find(|event| event.id == event_id).cloned())
        {
            return self.event_detail(&event, cx);
        }
        let mut list = div().w_full().flex().flex_col().gap_4();
        for event in &events {
            list = list.child(self.event_card(event, cx));
        }
        let open_context_menu =
            self.menu_event_id
                .zip(self.menu_position)
                .and_then(|(event_id, position)| {
                    events
                        .iter()
                        .find(|event| event.id == event_id)
                        .map(|event| self.event_context_menu(event, position, cx))
                });
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
            .relative()
            .image_cache(self.image_cache.clone())
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
            .children(open_context_menu)
            .into_any_element()
    }
}

struct CancelEventConfirmModal {
    clan_id: ClanId,
    event: ClanEventItem,
    settings: Entity<Settings>,
    focus_handle: FocusHandle,
    deleting: bool,
    delete_task: Option<Task<()>>,
}

impl Focusable for CancelEventConfirmModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CancelEventConfirmModal {
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.deleting {
            Shell::global(cx).update(cx, |shell, cx| shell.dismiss_modal(window, cx));
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.deleting {
            return;
        }
        self.deleting = true;
        cx.notify();
        let event_id = self.event.id;
        let task = EventsStore::global(cx).update(cx, |store, cx| {
            store.delete_event(self.event.clone(), self.clan_id, cx)
        });
        let window_handle = window.window_handle();
        self.delete_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let succeeded = result.is_ok();
            let _ = this.update(cx, |this, cx| {
                this.deleting = false;
                if let Err(error) = result {
                    tracing::warn!(%error, event_id, "failed to cancel event");
                }
                cx.notify();
            });
            if succeeded {
                let _ = window_handle.update(cx, |_, window, cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.dismiss_modal(window, cx));
                });
            }
        }));
    }
}

impl Render for CancelEventConfirmModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let tr = |key| mezon_i18n::t(&locale, key);
        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, window, cx| this.dismiss(window, cx)))
            .w(px(440.))
            .p_5()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_primary)
            .shadow_lg()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text_primary)
                    .child(tr("eventCreator.actions.cancelEventQuestion")),
            )
            .child(
                div()
                    .mt_4()
                    .text_color(theme.text_secondary)
                    .child(tr("eventCreator.actions.confirmCancelEvent")),
            )
            .child(
                div()
                    .mt_6()
                    .flex()
                    .justify_end()
                    .gap_3()
                    .child(
                        Button::new("never-mind-cancel-event")
                            .label(tr("eventCreator.actions.neverMind"))
                            .ghost()
                            .disabled(self.deleting)
                            .on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx))),
                    )
                    .child(
                        Button::new("confirm-cancel-event")
                            .label(tr("eventCreator.actions.cancelEvent"))
                            .danger()
                            .loading(self.deleting)
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    ),
            )
    }
}

fn open_cancel_event_confirm(
    clan_id: ClanId,
    event: ClanEventItem,
    settings: Entity<Settings>,
    window: &mut Window,
    cx: &mut App,
) {
    let modal = cx.new(|cx| CancelEventConfirmModal {
        clan_id,
        event,
        settings,
        focus_handle: cx.focus_handle(),
        deleting: false,
        delete_task: None,
    });
    Shell::global(cx).update(cx, |shell, cx| {
        shell.show_stacked_modal(modal.into(), window, cx)
    });
}

struct ShareEventModal {
    channel_name: String,
    link: String,
    settings: Entity<Settings>,
    focus_handle: FocusHandle,
    copied: bool,
    copy_reset_task: Option<Task<()>>,
}

impl Focusable for ShareEventModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ShareEventModal {
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Shell::global(cx).update(cx, |shell, cx| shell.dismiss_modal(window, cx));
    }

    fn reset_copy_after_leave(&mut self, cx: &mut Context<Self>) {
        if !self.copied {
            return;
        }
        self.copy_reset_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                this.copy_reset_task = None;
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for ShareEventModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let tr = |key| mezon_i18n::t(&locale, key);
        let link = self.link.clone();

        div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, window, cx| this.dismiss(window, cx)))
            .w(px(440.))
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_primary)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text_primary)
                    .child(tr("eventCreator.eventDetail.inviteFriends"))
                    .child(
                        div()
                            .id("close-share-event")
                            .cursor_pointer()
                            .p_1()
                            .on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx)))
                            .child(
                                Icon::new(IconName::CloseButton)
                                    .size(px(18.))
                                    .text_color(theme.text_secondary),
                            ),
                    ),
            )
            .child(
                div()
                    .mt_4()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(theme.text_primary)
                    .child(
                        Icon::new(IconName::Speaker)
                            .size(px(20.))
                            .text_color(theme.text_secondary),
                    )
                    .child(self.channel_name.clone()),
            )
            .child(
                div()
                    .mt_4()
                    .text_color(theme.text_secondary)
                    .child(tr("eventCreator.eventDetail.shareLink")),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.text_secondary)
                    .bg(theme.bg_floating)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text_primary)
                            .child(self.link.clone()),
                    )
                    .child(
                        div()
                            .id("copy-shared-event-link")
                            .group("copy-shared-event-link")
                            .size(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg_hover))
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                if !hovered {
                                    this.reset_copy_after_leave(cx);
                                }
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.copied {
                                    return;
                                }
                                cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                                this.copied = true;
                                this.copy_reset_task = None;
                                cx.notify();
                            }))
                            .child(
                                Icon::new(if self.copied {
                                    IconName::Tick
                                } else {
                                    IconName::CopyIcon
                                })
                                .size(px(18.))
                                .text_color(theme.text_secondary)
                                .group_hover("copy-shared-event-link", |style| {
                                    style.text_color(theme.text_primary)
                                }),
                            ),
                    ),
            )
    }
}

fn open_share_event_modal(
    channel_name: String,
    link: String,
    settings: Entity<Settings>,
    window: &mut Window,
    cx: &mut App,
) {
    let modal = cx.new(|cx| ShareEventModal {
        channel_name,
        link,
        settings,
        focus_handle: cx.focus_handle(),
        copied: false,
        copy_reset_task: None,
    });
    Shell::global(cx).update(cx, |shell, cx| {
        shell.show_stacked_modal(modal.into(), window, cx)
    });
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
