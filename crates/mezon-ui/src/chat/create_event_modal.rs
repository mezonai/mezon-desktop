use chrono::{Datelike, Duration, Local, TimeZone, Timelike};
use gpui::{
    AnyElement, App, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    PathPromptOptions, Render, SharedString, Subscription, Task, Window, div, img, prelude::*, px,
};
use mezon_store::{
    ChannelId, ChannelList, ChannelType, ClanEventItem, ClanId, ClanImageMimeType, ClanList,
    CreateEventDraft, EventsStore, MAX_CLAN_LOGO_BYTES, Settings, UpdateEventDraft,
};
use std::rc::Rc;

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, DatePicker, DatePickerEvent, Dropdown, DropdownPlacement,
    DropdownTriggerStyle, Icon, IconName, Input, InputEvent, InputState, TextArea, TextAreaEvent,
};
use crate::theme::ActiveTheme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Location,
    Details,
    Review,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LocationKind {
    Voice,
    Somewhere,
    External,
}

fn event_weekday_key(weekday: chrono::Weekday) -> &'static str {
    match weekday {
        chrono::Weekday::Mon => "eventCreator.fields.eventFrequency.weekday.mon",
        chrono::Weekday::Tue => "eventCreator.fields.eventFrequency.weekday.tue",
        chrono::Weekday::Wed => "eventCreator.fields.eventFrequency.weekday.wed",
        chrono::Weekday::Thu => "eventCreator.fields.eventFrequency.weekday.thu",
        chrono::Weekday::Fri => "eventCreator.fields.eventFrequency.weekday.fri",
        chrono::Weekday::Sat => "eventCreator.fields.eventFrequency.weekday.sat",
        chrono::Weekday::Sun => "eventCreator.fields.eventFrequency.weekday.sun",
    }
}

fn event_month_key(month0: u32) -> &'static str {
    [
        "common.dateTime.monthsShort.jan",
        "common.dateTime.monthsShort.feb",
        "common.dateTime.monthsShort.mar",
        "common.dateTime.monthsShort.apr",
        "common.dateTime.monthsShort.may",
        "common.dateTime.monthsShort.jun",
        "common.dateTime.monthsShort.jul",
        "common.dateTime.monthsShort.aug",
        "common.dateTime.monthsShort.sep",
        "common.dateTime.monthsShort.oct",
        "common.dateTime.monthsShort.nov",
        "common.dateTime.monthsShort.dec",
    ][month0 as usize]
}

fn event_repeat_labels(locale: &str, date: chrono::NaiveDate) -> Vec<SharedString> {
    let t = |key| mezon_i18n::t(locale, key).to_string();
    let weekday = t(event_weekday_key(date.weekday()));
    let occurrence_key = match (date.day() - 1) / 7 {
        0 => "eventCreator.fields.eventFrequency.occurrence.first",
        1 => "eventCreator.fields.eventFrequency.occurrence.second",
        2 => "eventCreator.fields.eventFrequency.occurrence.third",
        3 => "eventCreator.fields.eventFrequency.occurrence.fourth",
        _ => "eventCreator.fields.eventFrequency.occurrence.fifth",
    };
    let weekday_occurrence = t(occurrence_key);
    let month_day = format!("{} {}", date.day(), t(event_month_key(date.month0())));
    let mut labels = vec![
        t("eventCreator.fields.eventFrequency.noRepeat"),
        t("eventCreator.fields.eventFrequency.weeklyOn").replace("{{name}}", &weekday),
        t("eventCreator.fields.eventFrequency.everyOther").replace("{{name}}", &weekday),
        t("eventCreator.fields.eventFrequency.monthlyOn")
            .replace("{{name}}", &format!("{weekday_occurrence} {weekday}")),
        t("eventCreator.fields.eventFrequency.annuallyOn").replace("{{name}}", &month_day),
    ];
    if !matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun) {
        labels.push(t("eventCreator.fields.eventFrequency.everyWeekday"));
    }
    labels.into_iter().map(Into::into).collect()
}

pub enum EventSelectEvent {
    Change(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventSelectPlacement {
    Up,
    Down,
}

struct EventSelect {
    id: ElementId,
    items: Rc<Vec<SharedString>>,
    icons: Rc<Vec<Option<IconName>>>,
    selected: Option<usize>,
    open: bool,
    placeholder: SharedString,
    no_results: SharedString,
    placement: EventSelectPlacement,
}

impl EventEmitter<EventSelectEvent> for EventSelect {}

impl EventSelect {
    fn new(id: impl Into<ElementId>, items: Vec<SharedString>) -> Self {
        Self {
            id: id.into(),
            items: Rc::new(items),
            icons: Rc::new(Vec::new()),
            selected: None,
            open: false,
            placeholder: SharedString::default(),
            no_results: SharedString::default(),
            placement: EventSelectPlacement::Down,
        }
    }

    fn placeholder(mut self, value: impl Into<SharedString>) -> Self {
        self.placeholder = value.into();
        self
    }

    fn no_results(mut self, value: impl Into<SharedString>) -> Self {
        self.no_results = value.into();
        self
    }

    fn placement(mut self, value: EventSelectPlacement) -> Self {
        self.placement = value;
        self
    }

    fn icons(mut self, value: Vec<Option<IconName>>) -> Self {
        self.icons = Rc::new(value);
        self
    }

    fn selected(&self) -> Option<usize> {
        self.selected
    }

    fn value(&self) -> Option<&SharedString> {
        self.selected.and_then(|index| self.items.get(index))
    }

    fn set_selected(&mut self, value: Option<usize>, cx: &mut Context<Self>) {
        self.selected = value;
        cx.notify();
    }

    fn set_items(
        &mut self,
        items: Vec<SharedString>,
        icons: Vec<Option<IconName>>,
        cx: &mut Context<Self>,
    ) {
        self.items = Rc::new(items);
        self.icons = Rc::new(icons);
        if self.selected.is_some_and(|index| index >= self.items.len()) {
            self.selected = None;
        }
        cx.notify();
    }
}

impl Render for EventSelect {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let toggle_entity = cx.weak_entity();
        let close_entity = cx.weak_entity();
        let select_entity = cx.weak_entity();
        let next_open = !self.open;
        Dropdown::new(self.id.clone())
            .shared_items(self.items.clone())
            .shared_icons(self.icons.clone())
            .selected(self.selected)
            .open(self.open)
            .placeholder(self.placeholder.clone())
            .no_results(self.no_results.clone())
            .placement(match self.placement {
                EventSelectPlacement::Up => DropdownPlacement::Up,
                EventSelectPlacement::Down => DropdownPlacement::Down,
            })
            .trigger_style(DropdownTriggerStyle::InputPrimary)
            .trigger_background(theme.bg_secondary.into())
            .popup_background(theme.bg_primary.into())
            .on_toggle(move |_, cx| {
                toggle_entity
                    .update(cx, |select, cx| {
                        select.open = next_open;
                        cx.notify();
                    })
                    .ok();
            })
            .on_close(move |_, cx| {
                close_entity
                    .update(cx, |select, cx| {
                        select.open = false;
                        cx.notify();
                    })
                    .ok();
            })
            .on_select(move |index, _, cx| {
                select_entity
                    .update(cx, |select, cx| {
                        select.selected = Some(index);
                        select.open = false;
                        cx.emit(EventSelectEvent::Change(index));
                        cx.notify();
                    })
                    .ok();
            })
    }
}

pub struct CreateEventModal {
    clan_id: ClanId,
    settings: Entity<Settings>,
    step: Step,
    location_kind: Option<LocationKind>,
    voice_channels: Vec<(ChannelId, String)>,
    audience_channels: Vec<(ChannelId, String, ChannelType, bool)>,
    voice_select: Entity<EventSelect>,
    audience_select: Entity<EventSelect>,
    repeat_select: Entity<EventSelect>,
    start_time: Entity<EventSelect>,
    end_time: Entity<EventSelect>,
    time_values: Vec<u32>,
    start_date: Entity<DatePicker>,
    end_date: Entity<DatePicker>,
    address: Entity<InputState>,
    topic: Entity<InputState>,
    topic_dirty: bool,
    description: Entity<TextArea>,
    focus_handle: FocusHandle,
    creating: bool,
    cover_url: Option<SharedString>,
    uploading_cover: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
    _create_task: Option<Task<()>>,
    _cover_task: Option<Task<()>>,
    original_event: Option<ClanEventItem>,
}

impl Focusable for CreateEventModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CreateEventModal {
    pub fn new(
        clan_id: ClanId,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let channels_entity = ChannelList::global(cx);
        channels_entity.update(cx, |channels, cx| channels.load_for_clan(clan_id, cx));
        let channels = channels_entity.read(cx);
        let mut seen = std::collections::HashSet::new();
        let mut voice_channels = Vec::new();
        let mut audience_channels = Vec::new();
        for channel in channels
            .categories_for_clan(clan_id)
            .iter()
            .flat_map(|category| &category.channels)
        {
            if !seen.insert(channel.id) || !channel.visible_in_sidebar() {
                continue;
            }
            if channel.channel_type == ChannelType::Voice {
                voice_channels.push((channel.id, channel.name.clone()));
            }
            if channel.private
                && matches!(
                    channel.channel_type,
                    ChannelType::Text | ChannelType::Thread
                )
            {
                audience_channels.push((
                    channel.id,
                    channel.name.clone(),
                    channel.channel_type,
                    channel.private,
                ));
            }
        }
        let locale = settings.read(cx).language.clone();
        let t = |key| mezon_i18n::t(&locale, key).to_string();
        let voice_select = cx.new(|_| {
            EventSelect::new(
                "event-voice",
                voice_channels
                    .iter()
                    .map(|(_, name)| name.clone().into())
                    .collect(),
            )
            .placeholder(t("eventCreator.fields.VoiceChannel.title"))
            .no_results(t("invitation.noResults"))
            .placement(EventSelectPlacement::Down)
            .icons(vec![Some(IconName::Speaker); voice_channels.len()])
        });
        let audience_select = cx.new(|_| {
            EventSelect::new(
                "event-audience",
                audience_channels
                    .iter()
                    .map(|(_, name, _, _)| name.clone().into())
                    .collect(),
            )
            .placeholder(t("eventCreator.fields.channel.title"))
            .no_results(t("invitation.noResults"))
            .placement(EventSelectPlacement::Up)
            .icons(
                audience_channels
                    .iter()
                    .map(|(_, _, ty, private)| {
                        Some(match (ty, private) {
                            (ChannelType::Thread, true) => IconName::ThreadIconLocker,
                            (ChannelType::Thread, false) => IconName::ThreadIcon,
                            (_, true) => IconName::HashtagLocked,
                            _ => IconName::Hashtag,
                        })
                    })
                    .collect(),
            )
        });
        let now = Local::now();
        let repeat_labels = event_repeat_labels(&locale, now.date_naive());
        let repeat_select = cx.new(|_| {
            EventSelect::new("event-repeat", repeat_labels).placement(EventSelectPlacement::Down)
        });
        repeat_select.update(cx, |select, cx| select.set_selected(Some(0), cx));

        let mut start_at = now;
        if now.minute() != 0 || now.second() != 0 || now.nanosecond() != 0 {
            start_at += Duration::hours(1);
        }
        start_at = start_at
            .with_minute(0)
            .and_then(|value| value.with_second(0))
            .and_then(|value| value.with_nanosecond(0))
            .unwrap_or(start_at);
        let end_at = start_at + Duration::hours(1);
        let today = now.date_naive();
        let start_day = start_at.date_naive();
        let end_day = end_at.date_naive();
        let start_minutes = start_at.hour() * 60;
        let end_minutes = end_at.hour() * 60;
        let time_values: Vec<u32> = (0..96).map(|slot| slot * 15 * 60).collect();
        let time_labels: Vec<SharedString> = time_values
            .iter()
            .map(|seconds| format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60).into())
            .collect();
        let start_time = cx.new(|_| {
            EventSelect::new("event-start-time", time_labels.clone())
                .placement(EventSelectPlacement::Down)
        });
        let end_time = cx.new(|_| {
            EventSelect::new("event-end-time", time_labels).placement(EventSelectPlacement::Down)
        });
        start_time.update(cx, |select, cx| {
            select.set_selected(Some((start_minutes / 15) as usize), cx)
        });
        end_time.update(cx, |select, cx| {
            select.set_selected(Some((end_minutes / 15) as usize), cx)
        });
        let start_date = cx.new(|cx| {
            let mut picker = DatePicker::new(cx);
            picker.set_locale(locale.clone());
            picker
        });
        let end_date = cx.new(|cx| {
            let mut picker = DatePicker::new(cx);
            picker.set_locale(locale.clone());
            picker
        });
        start_date.update(cx, |picker, cx| {
            picker.set_field_height(40.0, cx);
            picker.set_min(Some(today), cx);
            picker.set_selected_silent(Some(start_day), cx);
        });
        end_date.update(cx, |picker, cx| {
            picker.set_field_height(40.0, cx);
            picker.set_min(Some(start_day), cx);
            picker.set_selected_silent(Some(end_day), cx);
        });

        let address = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("eventCreator.fields.address.placeholder"))
        });
        let topic = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("eventCreator.fields.eventName.placeholder"))
        });
        let description = cx.new(|cx| {
            TextArea::new(window, cx)
                .placeholder(t("eventCreator.fields.description.description"))
                .max_length(255)
                .min_height(px(88.))
                .max_visible_lines(5)
        });
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(&address, |this, _, _: &InputEvent, cx| {
            this.error = None;
            cx.notify();
        }));
        subscriptions.push(cx.subscribe(&topic, |this, _, _: &InputEvent, cx| {
            this.topic_dirty = true;
            this.error = None;
            cx.notify();
        }));
        subscriptions.push(cx.subscribe(&description, |_, _, _: &TextAreaEvent, cx| cx.notify()));
        subscriptions.push(
            cx.subscribe(&voice_select, |this, _, _: &EventSelectEvent, cx| {
                this.error = None;
                cx.notify();
            }),
        );
        subscriptions.push(
            cx.subscribe(&audience_select, |_, _, _: &EventSelectEvent, cx| {
                cx.notify()
            }),
        );
        subscriptions.push(
            cx.subscribe(&repeat_select, |this, _, _: &EventSelectEvent, cx| {
                this.error = None;
                cx.notify();
            }),
        );
        subscriptions.push(
            cx.subscribe(&start_time, |this, _, _: &EventSelectEvent, cx| {
                this.error = None;
                cx.notify();
            }),
        );
        subscriptions.push(
            cx.subscribe(&end_time, |this, _, _: &EventSelectEvent, cx| {
                this.error = None;
                cx.notify();
            }),
        );
        subscriptions.push(
            cx.subscribe(&start_date, |this, _, event: &DatePickerEvent, cx| {
                if let DatePickerEvent::Change(Some(date)) = event {
                    this.end_date.update(cx, |picker, cx| {
                        picker.set_min(Some(*date), cx);
                        if picker.selected().is_some_and(|end| end < *date) {
                            picker.set_selected_silent(Some(*date), cx);
                        }
                    });
                    let labels = event_repeat_labels(&this.settings.read(cx).language, *date);
                    this.repeat_select
                        .update(cx, |select, cx| select.set_items(labels, Vec::new(), cx));
                }
                this.error = None;
                cx.notify();
            }),
        );
        subscriptions.push(cx.subscribe(&end_date, |this, _, _: &DatePickerEvent, cx| {
            this.error = None;
            cx.notify();
        }));
        subscriptions.push(cx.observe(&channels_entity, |this, _, cx| {
            this.refresh_channels(cx);
        }));

        Self {
            clan_id,
            settings,
            step: Step::Location,
            location_kind: None,
            voice_channels,
            audience_channels,
            voice_select,
            audience_select,
            repeat_select,
            start_time,
            end_time,
            time_values,
            start_date,
            end_date,
            address,
            topic,
            topic_dirty: false,
            description,
            focus_handle: cx.focus_handle(),
            creating: false,
            cover_url: None,
            uploading_cover: false,
            error: None,
            _subscriptions: subscriptions,
            _create_task: None,
            _cover_task: None,
            original_event: None,
        }
    }

    pub fn new_for_edit(
        clan_id: ClanId,
        settings: Entity<Settings>,
        event: ClanEventItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut modal = Self::new(clan_id, settings, window, cx);
        modal.location_kind = Some(if event.is_private {
            LocationKind::External
        } else if !event.address.is_empty() {
            LocationKind::Somewhere
        } else {
            LocationKind::Voice
        });
        modal.voice_select.update(cx, |select, cx| {
            select.set_selected(
                event.channel_voice_id.and_then(|id| {
                    modal
                        .voice_channels
                        .iter()
                        .position(|(channel_id, _)| *channel_id == id)
                }),
                cx,
            )
        });
        modal.audience_select.update(cx, |select, cx| {
            select.set_selected(
                event.channel_id.and_then(|id| {
                    modal
                        .audience_channels
                        .iter()
                        .position(|(channel_id, _, _, _)| *channel_id == id)
                }),
                cx,
            )
        });
        modal.repeat_select.update(cx, |select, cx| {
            select.set_selected(
                Some(event.repeat_type.saturating_sub(1).max(0) as usize),
                cx,
            )
        });
        if let Some(start) = Local
            .timestamp_opt(event.start_time_seconds as i64, 0)
            .single()
        {
            let repeat_labels =
                event_repeat_labels(&modal.settings.read(cx).language, start.date_naive());
            modal.repeat_select.update(cx, |select, cx| {
                select.set_items(repeat_labels, Vec::new(), cx)
            });
            modal.start_date.update(cx, |picker, cx| {
                picker.set_selected_silent(Some(start.date_naive()), cx)
            });
            modal.start_time.update(cx, |select, cx| {
                select.set_selected(Some((start.hour() * 4 + start.minute() / 15) as usize), cx)
            });
        }
        if let Some(end) = Local
            .timestamp_opt(event.end_time_seconds as i64, 0)
            .single()
        {
            modal.end_date.update(cx, |picker, cx| {
                picker.set_selected_silent(Some(end.date_naive()), cx)
            });
            modal.end_time.update(cx, |select, cx| {
                select.set_selected(Some((end.hour() * 4 + end.minute() / 15) as usize), cx)
            });
        }
        modal
            .address
            .update(cx, |input, cx| input.set_value(&event.address, window, cx));
        modal
            .topic
            .update(cx, |input, cx| input.set_value(&event.title, window, cx));
        modal
            .description
            .update(cx, |input, cx| input.set_value(&event.description, cx));
        modal.cover_url = (!event.logo.is_empty()).then(|| event.logo.clone().into());
        modal.original_event = Some(event);
        modal
    }

    fn tr(&self, key: &'static str, cx: &App) -> String {
        mezon_i18n::t(&self.settings.read(cx).language, key).to_string()
    }

    fn refresh_channels(&mut self, cx: &mut Context<Self>) {
        let channels_entity = ChannelList::global(cx);
        let channels = channels_entity.read(cx);
        let mut seen = std::collections::HashSet::new();
        let mut voice_channels = Vec::new();
        let mut audience_channels = Vec::new();
        for channel in channels
            .categories_for_clan(self.clan_id)
            .iter()
            .flat_map(|category| &category.channels)
        {
            if !seen.insert(channel.id) || !channel.visible_in_sidebar() {
                continue;
            }
            if channel.channel_type == ChannelType::Voice {
                voice_channels.push((channel.id, channel.name.clone()));
            }
            if channel.private
                && matches!(
                    channel.channel_type,
                    ChannelType::Text | ChannelType::Thread
                )
            {
                audience_channels.push((
                    channel.id,
                    channel.name.clone(),
                    channel.channel_type,
                    channel.private,
                ));
            }
        }
        self.voice_channels = voice_channels;
        self.audience_channels = audience_channels;
        self.voice_select.update(cx, |select, cx| {
            select.set_items(
                self.voice_channels
                    .iter()
                    .map(|(_, name)| name.clone().into())
                    .collect(),
                vec![Some(IconName::Speaker); self.voice_channels.len()],
                cx,
            )
        });
        self.audience_select.update(cx, |select, cx| {
            select.set_items(
                self.audience_channels
                    .iter()
                    .map(|(_, name, _, _)| name.clone().into())
                    .collect(),
                self.audience_channels
                    .iter()
                    .map(|(_, _, ty, private)| {
                        Some(match (ty, private) {
                            (ChannelType::Thread, true) => IconName::ThreadIconLocker,
                            (ChannelType::Thread, false) => IconName::ThreadIcon,
                            (_, true) => IconName::HashtagLocked,
                            _ => IconName::Hashtag,
                        })
                    })
                    .collect(),
                cx,
            )
        });
        cx.notify();
    }
    fn return_to_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        super::clan_events_page::open_clan_events_modal(
            self.clan_id,
            self.settings.clone(),
            window,
            cx,
        );
    }
    fn valid_location(&self, cx: &App) -> bool {
        match self.location_kind {
            Some(LocationKind::Voice) => self.voice_select.read(cx).selected().is_some(),
            Some(LocationKind::Somewhere) => {
                let len = self.address.read(cx).value().trim().chars().count();
                len > 0 && len <= 100
            }
            Some(LocationKind::External) => true,
            None => false,
        }
    }
    fn timestamps(&self, cx: &App) -> Option<(u32, u32)> {
        let start_date = self.start_date.read(cx).selected()?;
        let end_date = self.end_date.read(cx).selected()?;
        let start_seconds = *self.time_values.get(self.start_time.read(cx).selected()?)?;
        let end_seconds = *self.time_values.get(self.end_time.read(cx).selected()?)?;
        let local_timestamp = |date: chrono::NaiveDate, seconds: u32| {
            let time = chrono::NaiveTime::from_num_seconds_from_midnight_opt(seconds, 0)?;
            Local
                .from_local_datetime(&date.and_time(time))
                .single()
                .map(|dt| dt.timestamp().max(0) as u32)
        };
        Some((
            local_timestamp(start_date, start_seconds)?,
            local_timestamp(end_date, end_seconds)?,
        ))
    }
    fn topic_is_valid(&self, cx: &App) -> bool {
        let topic = self.topic.read(cx).value().trim();
        !topic.is_empty()
            && !topic
                .chars()
                .any(|c| matches!(c, '`' | '<' | '>' | ',' | '/' | '"' | '\\' | '\''))
    }
    fn valid_details(&self, cx: &App) -> bool {
        self.timestamps(cx).is_some_and(|(start, end)| {
            self.topic_is_valid(cx)
                && (start > chrono::Utc::now().timestamp().max(0) as u32
                    || self
                        .original_event
                        .as_ref()
                        .is_some_and(|event| start == event.start_time_seconds))
                && end > start
        })
    }

    fn time_error(&self, cx: &App) -> Option<String> {
        let (start, end) = self.timestamps(cx)?;
        if start <= chrono::Utc::now().timestamp().max(0) as u32
            && self
                .original_event
                .as_ref()
                .is_none_or(|event| start != event.start_time_seconds)
        {
            Some(self.tr("eventCreator.errorMessages.startTimeFuture", cx))
        } else if end <= start {
            Some(self.tr("eventCreator.errorMessages.endTimeAfterStart", cx))
        } else {
            None
        }
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        match self.step {
            Step::Location if self.valid_location(cx) => self.step = Step::Details,
            Step::Details if self.valid_details(cx) => self.step = Step::Review,
            Step::Location => self.error = Some(self.tr("eventCreator.notify.type", cx).into()),
            Step::Details => {}
            Step::Review => {}
        }
        cx.notify();
    }
    fn back(&mut self, cx: &mut Context<Self>) {
        self.step = match self.step {
            Step::Location => Step::Location,
            Step::Details => Step::Location,
            Step::Review => Step::Details,
        };
        self.error = None;
        cx.notify();
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating || !self.valid_location(cx) || !self.valid_details(cx) {
            return;
        }
        let (start, end) = self.timestamps(cx).unwrap();
        let draft = CreateEventDraft {
            title: self.topic.read(cx).value().trim().to_string(),
            logo: self.cover_url.as_deref().unwrap_or_default().to_string(),
            description: self.description.read(cx).value().trim().to_string(),
            clan_id: self.clan_id,
            channel_voice_id: (self.location_kind == Some(LocationKind::Voice))
                .then(|| {
                    self.voice_select
                        .read(cx)
                        .selected()
                        .and_then(|i| self.voice_channels.get(i))
                        .map(|v| v.0)
                })
                .flatten(),
            address: if self.location_kind == Some(LocationKind::Somewhere) {
                self.address.read(cx).value().trim().to_string()
            } else {
                String::new()
            },
            start_time_seconds: start,
            end_time_seconds: end,
            channel_id: if self.location_kind == Some(LocationKind::External) {
                None
            } else {
                self.audience_select
                    .read(cx)
                    .selected()
                    .and_then(|i| self.audience_channels.get(i))
                    .map(|v| v.0)
            },
            repeat_type: self.repeat_select.read(cx).selected().unwrap_or(0) as i32 + 1,
            is_private: self.location_kind == Some(LocationKind::External),
        };
        self.creating = true;
        cx.notify();
        let task = if let Some(original) = self.original_event.clone() {
            let update = UpdateEventDraft {
                event_id: original.id,
                clan_id: self.clan_id,
                creator_id: original.creator_id,
                channel_id_old: original.channel_id,
                title: draft.title,
                logo: draft.logo,
                description: draft.description,
                channel_voice_id: draft.channel_voice_id,
                address: draft.address,
                start_time_seconds: draft.start_time_seconds,
                end_time_seconds: draft.end_time_seconds,
                channel_id: draft.channel_id,
                repeat_type: draft.repeat_type,
            };
            EventsStore::global(cx).update(cx, |store, cx| store.update_event(update, cx))
        } else {
            EventsStore::global(cx).update(cx, |store, cx| store.create_event(draft, cx))
        };
        let window_handle = window.window_handle();
        let clan_id = self.clan_id;
        let settings = self.settings.clone();
        self._create_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let succeeded = result.is_ok();
            let _ = this.update(cx, |this, cx| {
                this.creating = false;
                if let Err(error) = result {
                    this.error = Some(error.to_string().into());
                    cx.notify();
                }
            });
            if succeeded {
                let _ = window_handle.update(cx, move |_, window, cx| {
                    super::clan_events_page::open_clan_events_modal(
                        clan_id,
                        settings.clone(),
                        window,
                        cx,
                    );
                });
            }
        }));
    }

    fn pick_cover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.uploading_cover {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr("eventCreator.actions.uploadImage", cx).into()),
        });
        let window_handle = window.window_handle();
        self.uploading_cover = true;
        self.error = None;
        cx.notify();
        self._cover_task = Some(cx.spawn(async move |this, cx| {
            let path = crate::util::file_dialog::resolve(rx, cx)
                .await
                .and_then(|paths| paths.into_iter().next());
            let Some(path) = path else {
                let _ = this.update(cx, |this, cx| {
                    this.uploading_cover = false;
                    cx.notify();
                });
                return;
            };
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let size = cx
                .background_spawn({
                    let path = path.clone();
                    async move { std::fs::metadata(path).ok().map(|m| m.len()) }
                })
                .await;
            if size.is_some_and(|size| size > MAX_CLAN_LOGO_BYTES) {
                let labels = this.update(cx, |this, cx| {
                    this.uploading_cover = false;
                    cx.notify();
                    (
                        this.tr("common.filesTooPowerful", cx),
                        this.tr("common.maxFileSize", cx)
                            .replace("{{sizeLimit}}", "1 MB"),
                    )
                });
                if let Ok((title, content)) = labels {
                    let _ = window_handle.update(cx, |_, window, cx| {
                        Shell::global(cx).update(cx, |shell, cx| {
                            shell.show_upload_limit(title, content, window, cx);
                        });
                    });
                }
                return;
            }
            if !ClanImageMimeType::is_allowed_extension(&extension) {
                let _ = this.update(cx, |this, cx| {
                    this.uploading_cover = false;
                    this.error = Some(
                        this.tr("eventCreator.errorMessages.unsupportedImageType", cx)
                            .into(),
                    );
                    cx.notify();
                });
                return;
            }
            let upload = match this.update(cx, |_, cx| {
                ClanList::global(cx).update(cx, |store, cx| {
                    store.upload_clan_image(&path, MAX_CLAN_LOGO_BYTES, cx)
                })
            }) {
                Ok(task) => task,
                Err(_) => return,
            };
            let result = upload.await;
            let _ = window_handle.update(cx, |_, _window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.uploading_cover = false;
                    match result {
                        Ok(url) => this.cover_url = Some(url.into()),
                        Err(error) => this.error = Some(error.into()),
                    }
                    cx.notify();
                });
            });
        }));
    }

    fn progress(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let current = match self.step {
            Step::Location => 0,
            Step::Details => 1,
            Step::Review => 2,
        };
        [
            "eventCreator.tabs.location",
            "eventCreator.tabs.eventInfo",
            "eventCreator.tabs.review",
        ]
        .into_iter()
        .enumerate()
        .fold(div().flex().gap_4(), |row, (index, key)| {
            row.child(
                div()
                    .id("dismiss-event-upload-limit")
                    .flex_1()
                    .child(div().h(px(6.)).rounded_full().bg(if index == current {
                        gpui::rgb(0x959cf7)
                    } else {
                        theme.text_muted
                    }))
                    .child(
                        div()
                            .mt_2()
                            .text_size(px(10.))
                            .text_color(if index == current {
                                theme.text_secondary
                            } else {
                                theme.text_muted
                            })
                            .child(self.tr(key, cx)),
                    ),
            )
        })
        .into_any_element()
    }
    fn option(
        &self,
        kind: LocationKind,
        icon: IconName,
        title: &'static str,
        description: &'static str,
        enabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let selected = self.location_kind == Some(kind);
        div()
            .id(SharedString::from(format!("event-type-{}", kind as u8)))
            .w_full()
            .px_3()
            .py_2()
            .rounded(px(5.))
            .border_1()
            .border_color(if selected { theme.brand } else { theme.border })
            .bg(theme.bg_secondary)
            .flex()
            .items_center()
            .justify_between()
            .when(enabled, |option| {
                option
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.location_kind = Some(kind);
                        this.error = None;
                        cx.notify();
                    }))
            })
            .when(!enabled, |option| option.cursor_not_allowed().opacity(0.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(if icon == IconName::Location {
                        img(IconName::Location.path())
                            .size(px(23.))
                            .into_any_element()
                    } else {
                        Icon::new(icon)
                            .size(px(23.))
                            .text_color(theme.text_secondary)
                            .into_any_element()
                    })
                    .child(
                        div()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.tr(title, cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme.text_secondary)
                                    .child(self.tr(description, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .size(px(14.))
                    .rounded_full()
                    .border_2()
                    .border_color(if selected {
                        theme.brand
                    } else {
                        theme.text_secondary
                    })
                    .when(selected, |d| d.bg(theme.brand)),
            )
            .into_any_element()
    }
    fn required_label(&self, key: &'static str, cx: &Context<Self>) -> AnyElement {
        div()
            .mb_1()
            .flex()
            .gap_1()
            .text_size(px(14.))
            .font_weight(FontWeight::BOLD)
            .child(self.tr(key, cx))
            .child(div().text_color(cx.theme().danger_text).child("*"))
            .into_any_element()
    }
    fn input_field(
        &self,
        key: &'static str,
        input: Entity<InputState>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        div()
            .w_full()
            .child(self.required_label(key, cx))
            .child(
                div()
                    .h(px(38.))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .child(Input::new(&input)),
            )
            .into_any_element()
    }
    fn date_field(
        &self,
        key: &'static str,
        picker: Entity<DatePicker>,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .flex_1()
            .child(self.required_label(key, cx))
            .child(picker)
            .into_any_element()
    }
    fn time_field(
        &self,
        key: &'static str,
        select: Entity<EventSelect>,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .flex_1()
            .child(self.required_label(key, cx))
            .child(select)
            .into_any_element()
    }

    fn location_content(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let editing_private = self
            .original_event
            .as_ref()
            .is_some_and(|event| event.is_private);
        let editing_non_private = self
            .original_event
            .as_ref()
            .is_some_and(|event| !event.is_private);
        let can_select_audience = self
            .original_event
            .as_ref()
            .is_none_or(|event| !event.is_private);
        div()
            .child(
                div()
                    .text_center()
                    .mb_4()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.tr("eventCreator.screens.eventType.title", cx)),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme.text_secondary)
                            .child(self.tr("eventCreator.screens.eventType.subtitle", cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(!editing_private, |options| {
                        options
                            .child(self.option(
                                LocationKind::Voice,
                                IconName::Speaker,
                                "eventCreator.fields.channelType.voiceChannel.title",
                                "eventCreator.fields.channelType.voiceChannel.description",
                                self.original_event.is_some() || !self.voice_channels.is_empty(),
                                cx,
                            ))
                            .child(self.option(
                                LocationKind::Somewhere,
                                IconName::Location,
                                "eventCreator.fields.channelType.somewhere.title",
                                "eventCreator.fields.channelType.somewhere.description",
                                true,
                                cx,
                            ))
                    })
                    .when(!editing_non_private, |options| {
                        options.child(self.option(
                            LocationKind::External,
                            IconName::Speaker,
                            "eventCreator.fields.channelType.privateEvent.title",
                            "eventCreator.fields.channelType.privateEvent.description",
                            true,
                            cx,
                        ))
                    }),
            )
            .when(self.location_kind == Some(LocationKind::Voice), |d| {
                d.child(div().mt_3().child(self.voice_select.clone()))
            })
            .when(self.location_kind == Some(LocationKind::Somewhere), |d| {
                d.child(div().mt_3().child(self.input_field(
                    "eventCreator.fields.address.title",
                    self.address.clone(),
                    cx,
                )))
            })
            .when(
                self.location_kind != Some(LocationKind::External) && can_select_audience,
                |d| {
                    d.child(
                        div()
                            .text_center()
                            .mt_4()
                            .mb_2()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(
                                        self.tr("eventCreator.screens.channelSelection.title", cx),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .text_color(theme.text_secondary)
                                    .child(self.tr(
                                        "eventCreator.screens.channelSelection.description",
                                        cx,
                                    )),
                            ),
                    )
                    .child(self.audience_select.clone())
                    .when(
                        self.audience_select.read(cx).selected().is_some(),
                        |d| {
                            d.child(
                                div()
                                    .id("clear-event-audience")
                                    .mt_1()
                                    .flex()
                                    .justify_end()
                                    .text_size(px(13.))
                                    .text_color(theme.brand)
                                    .cursor_pointer()
                                    .hover(|style| style.underline())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.audience_select
                                            .update(cx, |select, cx| select.set_selected(None, cx));
                                        cx.notify();
                                    }))
                                    .child(self.tr("eventCreator.actions.clearAudiences", cx)),
                            )
                        },
                    )
                },
            )
            .into_any_element()
    }
    fn details_content(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        div()
            .child(self.input_field(
                "eventCreator.fields.eventName.title",
                self.topic.clone(),
                cx,
            ))
            .when(self.topic_dirty && !self.topic_is_valid(cx), |d| {
                d.child(
                    div()
                        .mt_1()
                        .text_size(px(12.))
                        .italic()
                        .text_color(theme.danger_text)
                        .child(self.tr("eventCreator.errorMessages.invalidTopic", cx)),
                )
            })
            .child(
                div()
                    .flex()
                    .gap_3()
                    .mt_3()
                    .child(self.date_field(
                        "eventCreator.fields.startDate.title",
                        self.start_date.clone(),
                        cx,
                    ))
                    .child(self.time_field(
                        "eventCreator.fields.startTime.title",
                        self.start_time.clone(),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .mt_3()
                    .child(self.date_field(
                        "eventCreator.fields.endDate.title",
                        self.end_date.clone(),
                        cx,
                    ))
                    .child(self.time_field(
                        "eventCreator.fields.endTime.title",
                        self.end_time.clone(),
                        cx,
                    )),
            )
            .child(
                div()
                    .mt_3()
                    .child(
                        div()
                            .mb_1()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .child(self.tr("eventCreator.fields.eventFrequency.title", cx)),
                    )
                    .child(self.repeat_select.clone()),
            )
            .when_some(self.time_error(cx), |d, error| {
                d.child(
                    div()
                        .mt_1()
                        .text_size(px(12.))
                        .italic()
                        .text_color(theme.danger_text)
                        .child(error),
                )
            })
            .child(
                div()
                    .mt_3()
                    .child(
                        div()
                            .mb_1()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .child(self.tr("eventCreator.fields.description.title", cx)),
                    )
                    .child(
                        div()
                            .relative()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .child(self.description.clone())
                            .child(
                                div()
                                    .absolute()
                                    .right(px(10.))
                                    .bottom(px(8.))
                                    .text_size(px(12.))
                                    .text_color(theme.text_muted)
                                    .child(
                                        (255usize.saturating_sub(
                                            self.description.read(cx).value().chars().count(),
                                        ))
                                        .to_string(),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .mt_3()
                    .child(
                        div()
                            .mb_1()
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .child(self.tr("eventCreator.fields.cover", cx)),
                    )
                    .child(
                        Button::new("upload-event-cover")
                            .label(self.tr("eventCreator.actions.uploadImage", cx))
                            .primary()
                            .loading(self.uploading_cover)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_cover(window, cx)),
                            ),
                    )
                    .when_some(self.cover_url.clone(), |d, url| {
                        d.child(
                            div()
                                .mt_2()
                                .relative()
                                .w_full()
                                .h(px(180.))
                                .rounded(px(5.))
                                .overflow_hidden()
                                .child(
                                    img(url)
                                        .absolute()
                                        .inset_0()
                                        .size_full()
                                        .object_fit(gpui::ObjectFit::Cover),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
    fn review_content(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let start = self
            .timestamps(cx)
            .and_then(|(start, _)| chrono::DateTime::from_timestamp(start as i64, 0))
            .map(|date| {
                date.with_timezone(&Local)
                    .format("%a, %b %-d - %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        let location = match self.location_kind {
            Some(LocationKind::Voice) => self
                .voice_select
                .read(cx)
                .value()
                .map(ToString::to_string)
                .unwrap_or_default(),
            Some(LocationKind::Somewhere) => self.address.read(cx).value().to_string(),
            _ => self.tr("eventCreator.eventDetail.privateRoom", cx),
        };
        let event_type = if self.location_kind == Some(LocationKind::External) {
            self.tr("eventCreator.eventDetail.privateEvent", cx)
        } else if self.audience_select.read(cx).selected().is_some() {
            self.tr("eventCreator.eventDetail.channelEvent", cx)
        } else {
            self.tr("eventCreator.eventDetail.clanEvent", cx)
        };
        let audience_note = if self.location_kind == Some(LocationKind::External) {
            self.tr("eventCreator.eventDetail.onlyInvitedMembers", cx)
        } else if let Some(index) = self.audience_select.read(cx).selected() {
            let (_, name, ty, _) = &self.audience_channels[index];
            format!(
                "{} {}{}",
                self.tr("eventCreator.eventDetail.audienceConsists", cx),
                if *ty == ChannelType::Thread {
                    self.tr("eventCreator.eventDetail.thread", cx)
                } else {
                    self.tr("eventCreator.eventDetail.channel", cx)
                },
                name
            )
        } else {
            self.tr("eventMenu.dashboard.noti", cx)
        };
        div()
            .child(
                div()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .overflow_hidden()
                    .bg(theme.surfaces.primary)
                    .when_some(self.cover_url.clone(), |d, url| {
                        d.child(
                            div()
                                .relative()
                                .w_full()
                                .h(px(180.))
                                .overflow_hidden()
                                .child(
                                    img(url)
                                        .absolute()
                                        .inset_0()
                                        .size_full()
                                        .object_fit(gpui::ObjectFit::Cover),
                                ),
                        )
                    })
                    .child(
                        div()
                            .p_4()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Icon::new(IconName::IconEvents)
                                            .size(px(24.))
                                            .text_color(theme.text_secondary),
                                    )
                                    .child(div().font_weight(FontWeight::SEMIBOLD).child(start))
                                    .child(
                                        div()
                                            .px_1()
                                            .py(px(2.))
                                            .rounded(px(2.))
                                            .bg(
                                                if self.location_kind
                                                    == Some(LocationKind::External)
                                                {
                                                    gpui::rgb(0xef4444)
                                                } else if self
                                                    .audience_select
                                                    .read(cx)
                                                    .selected()
                                                    .is_some()
                                                {
                                                    gpui::rgb(0xf97316)
                                                } else {
                                                    gpui::rgb(0x3b82f6)
                                                },
                                            )
                                            .text_color(gpui::white())
                                            .text_size(px(13.))
                                            .child(event_type),
                                    ),
                            )
                            .child(
                                div()
                                    .mt_4()
                                    .font_weight(FontWeight::BOLD)
                                    .child(self.topic.read(cx).value().to_string()),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_color(theme.text_secondary)
                                    .child(self.description.read(cx).value().to_string()),
                            ),
                    )
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_t_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(if self.location_kind == Some(LocationKind::Somewhere) {
                                img(IconName::Location.path())
                                    .size(px(21.))
                                    .into_any_element()
                            } else {
                                Icon::new(IconName::Speaker)
                                    .size(px(21.))
                                    .text_color(theme.text_secondary)
                                    .into_any_element()
                            })
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(location)),
                    )
                    .child(
                        div()
                            .px_4()
                            .pb_3()
                            .text_size(px(13.))
                            .text_color(theme.text_secondary)
                            .child(audience_note),
                    ),
            )
            .child(
                div()
                    .mt_6()
                    .text_center()
                    .child(
                        div()
                            .text_size(px(19.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.tr("eventCreator.screens.eventPreview.title", cx)),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme.text_secondary)
                            .child(self.tr("eventCreator.screens.eventPreview.subtitle", cx)),
                    ),
            )
            .into_any_element()
    }
}

impl Render for CreateEventModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_review = self.step == Step::Review;
        let is_edit = self.original_event.is_some();
        let can_continue = if self.step == Step::Location {
            self.valid_location(cx)
        } else {
            self.valid_details(cx)
        };
        let content = match self.step {
            Step::Location => self.location_content(cx),
            Step::Details => self.details_content(cx),
            Step::Review => self.review_content(cx),
        };
        let footer = div()
            .mt_5()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .when(self.step != Step::Location, |d| {
                        d.child(
                            Button::new("back-create-event")
                                .label(self.tr("eventCreator.actions.back", cx))
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                        )
                    })
                    .child(div()),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        Button::new("cancel-create-event")
                            .label(self.tr("eventCreator.actions.cancel", cx))
                            .disabled(self.creating)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if !this.creating {
                                    this.return_to_events(window, cx)
                                }
                            })),
                    )
                    .child(
                        Button::new("next-create-event")
                            .label(if is_review {
                                self.tr(
                                    if is_edit {
                                        "eventCreator.actions.edit"
                                    } else {
                                        "eventCreator.actions.create"
                                    },
                                    cx,
                                )
                            } else {
                                self.tr("eventCreator.actions.next", cx)
                            })
                            .primary()
                            .disabled(!can_continue || self.creating)
                            .loading(self.creating)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if is_review {
                                    this.create(window, cx)
                                } else {
                                    this.next(cx)
                                }
                            })),
                    ),
            );
        let card = div()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, window, cx| {
                if !this.creating {
                    this.return_to_events(window, cx)
                }
            }))
            .relative()
            .w(px(520.))
            .max_h(gpui::relative(0.9))
            .flex()
            .flex_col()
            .rounded_lg()
            .overflow_hidden()
            .bg(theme.bg_primary)
            .text_color(theme.text_primary)
            .p_5()
            .child(self.progress(cx))
            .child(
                div()
                    .id("create-event-scroll")
                    .max_h(px(520.))
                    .flex_shrink_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .mt_6()
                    .px_3()
                    .child(content)
                    .when_some(self.error.clone(), |d, error| {
                        d.child(
                            div()
                                .mt_2()
                                .text_size(px(12.))
                                .text_color(theme.danger_text)
                                .child(error),
                        )
                    }),
            )
            .child(footer);

        div()
            .absolute()
            .inset_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .child(card)
    }
}

pub fn open_create_event_modal(
    clan_id: ClanId,
    settings: Entity<Settings>,
    window: &mut Window,
    cx: &mut App,
) {
    let modal = cx.new(|cx| CreateEventModal::new(clan_id, settings, window, cx));
    let focus_handle = modal.read(cx).focus_handle.clone();
    window.focus(&focus_handle, cx);
    Shell::global(cx).update(cx, |shell, cx| {
        shell.show_fullscreen_modal(modal.into(), cx)
    });
}

pub fn open_edit_event_modal(
    clan_id: ClanId,
    settings: Entity<Settings>,
    event: ClanEventItem,
    window: &mut Window,
    cx: &mut App,
) {
    let modal = cx.new(|cx| CreateEventModal::new_for_edit(clan_id, settings, event, window, cx));
    let focus_handle = modal.read(cx).focus_handle.clone();
    window.focus(&focus_handle, cx);
    Shell::global(cx).update(cx, |shell, cx| {
        shell.show_fullscreen_modal(modal.into(), cx)
    });
}

#[cfg(test)]
mod tests {
    use super::event_repeat_labels;
    use chrono::NaiveDate;

    #[test]
    fn repeat_labels_match_the_selected_weekday_and_occurrence() {
        let friday = NaiveDate::from_ymd_opt(2026, 8, 28).expect("valid date");
        let labels = event_repeat_labels("en", friday);

        assert_eq!(labels[1], "Weekly on Friday");
        assert_eq!(labels[2], "Every other Friday");
        assert_eq!(labels[3], "Monthly on Fourth Friday");
        assert_eq!(labels[4], "Annually on 28 Aug");
        assert_eq!(labels[5], "Every weekday (Monday to Friday)");
    }

    #[test]
    fn weekend_repeat_labels_exclude_every_weekday() {
        let saturday = NaiveDate::from_ymd_opt(2026, 8, 29).expect("valid date");
        let labels = event_repeat_labels("en", saturday);

        assert_eq!(labels[1], "Weekly on Saturday");
        assert_eq!(labels[3], "Monthly on Fifth Saturday");
        assert_eq!(labels.len(), 5);
    }
}
