use std::rc::Rc;

use chrono::{Datelike, NaiveDate, Utc};
use gpui::{
    AnyElement, App, Context, Entity, EntityId, FocusHandle, FontWeight, MouseButton, SharedString,
    Subscription, Window, deferred, div, img, prelude::*, px, relative, rgb, uniform_list,
};
use mezon_store::{
    AccountEvent, AccountStore, Channel, ChannelId, ChannelType, ClanId, Settings, dob_needs_entry,
    is_adult_dob, schedule_settings_save,
};

use crate::app::shell::Shell;
use crate::components::primitives::{Icon, IconName, h_flex, v_flex};
use crate::router::{Route, navigate};
use crate::theme::ActiveTheme;
use crate::util::assets::AGE_RESTRICTED_WARNING;

const AGE_RESTRICTED_ON: i32 = 1;
const COLOR_DANGER: u32 = 0xDA373C;
const FIELD_BG: u32 = 0x2F3746;
const FIELD_BORDER: u32 = 0x3D4656;
const FIELD_TEXT: u32 = 0xD7DEEA;
const FIELD_TEXT_ACTIVE: u32 = 0xFFFFFF;
const FIELD_ITEM_HOVER: u32 = 0x3C4658;
const SUBMIT_BG: u32 = 0x2563EB;
const SUBMIT_DISABLED_BG: u32 = 0x9CA3AF;
const SUBMIT_DISABLED_TEXT: u32 = 0x4B5563;
const CARD_WIDTH: f32 = 550.;
const WARNING_SIZE: f32 = 200.;
const FIELD_ITEM_HEIGHT: f32 = 40.;
const FIELD_LIST_MAX_HEIGHT: f32 = 184.;
const BIRTH_YEAR_SPAN: i32 = 120;
const EARLIEST_BIRTH_YEAR: i32 = 1970;
const MONTH_KEYS: [&str; 12] = [
    "ageRestricted.month.january",
    "ageRestricted.month.february",
    "ageRestricted.month.march",
    "ageRestricted.month.april",
    "ageRestricted.month.may",
    "ageRestricted.month.june",
    "ageRestricted.month.july",
    "ageRestricted.month.august",
    "ageRestricted.month.september",
    "ageRestricted.month.october",
    "ageRestricted.month.november",
    "ageRestricted.month.december",
];

pub fn age_gate_blocks(channel: &Channel, cx: &App) -> bool {
    gated_channel_type(channel.age_restricted, channel.channel_type)
        && !viewer_is_adult(cx)
        && !channel_confirmed(channel.id, cx)
}

fn gated_channel_type(age_restricted: i32, channel_type: ChannelType) -> bool {
    age_restricted == AGE_RESTRICTED_ON
        && !matches!(channel_type, ChannelType::Voice | ChannelType::Stream)
}

fn birthday_from_parts(
    newest_year: i32,
    day: Option<usize>,
    month: Option<usize>,
    year: Option<usize>,
) -> Option<NaiveDate> {
    let day = u32::try_from(day? + 1).ok()?;
    let month = u32::try_from(month? + 1).ok()?;
    let year = newest_year - i32::try_from(year?).ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn channel_confirmed(channel_id: ChannelId, cx: &App) -> bool {
    Settings::try_global(cx).is_some_and(|settings| {
        settings
            .read(cx)
            .age_restricted_confirmed
            .contains(&channel_id)
    })
}

fn viewer_dob_seconds(cx: &App) -> Option<u32> {
    AccountStore::try_global(cx)
        .and_then(|store| store.read(cx).account.as_ref().map(|user| user.dob_seconds))
}

fn viewer_is_adult(cx: &App) -> bool {
    viewer_dob_seconds(cx).is_some_and(is_adult_dob)
}

fn viewer_needs_birthday(cx: &App) -> bool {
    AccountStore::try_global(cx).is_some_and(|store| {
        let store = store.read(cx);
        store.account_fetched()
            && store
                .account
                .as_ref()
                .is_some_and(|user| dob_needs_entry(user.dob_seconds))
    })
}

pub struct AgeRestrictedGate {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    birthday_modal: Option<EntityId>,
    birthday_prompt_dismissed: bool,
    birthday_prompt_pending: bool,
}

impl AgeRestrictedGate {
    pub fn new(clan_id: ClanId, channel_id: ChannelId, settings: Entity<Settings>) -> Self {
        Self {
            clan_id,
            channel_id,
            settings,
            birthday_modal: None,
            birthday_prompt_dismissed: false,
            birthday_prompt_pending: false,
        }
    }

    pub fn is_for(&self, clan_id: ClanId, channel_id: ChannelId) -> bool {
        self.clan_id == clan_id && self.channel_id == channel_id
    }

    pub fn sync_birthday_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !viewer_needs_birthday(cx) {
            self.dismiss_birthday_prompt(cx);
            return;
        }
        if let Some(shown) = self.birthday_modal {
            if Shell::global(cx).read(cx).modal_view_id() == Some(shown) {
                return;
            }
            self.birthday_modal = None;
            self.birthday_prompt_dismissed = true;
            return;
        }
        if self.birthday_prompt_dismissed || self.birthday_prompt_pending {
            return;
        }
        self.birthday_prompt_pending = true;
        cx.defer_in(window, |gate, window, cx| {
            gate.birthday_prompt_pending = false;
            if gate.birthday_modal.is_some()
                || gate.birthday_prompt_dismissed
                || !viewer_needs_birthday(cx)
            {
                return;
            }
            let settings = gate.settings.clone();
            let modal = cx.new(|cx| ConfirmBirthdayModal::new(settings, cx));
            let focus_handle = modal.read(cx).focus_handle.clone();
            window.focus(&focus_handle, cx);
            gate.birthday_modal = Some(modal.entity_id());
            Shell::global(cx).update(cx, |shell, cx| {
                shell.show_modal_keyboard_dismiss_only(modal.into(), cx)
            });
        });
    }

    pub fn reopen_birthday_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.birthday_prompt_dismissed = false;
        self.birthday_modal = None;
        self.sync_birthday_prompt(window, cx);
    }

    pub fn dismiss_birthday_prompt(&mut self, cx: &mut App) {
        self.birthday_prompt_dismissed = false;
        self.birthday_prompt_pending = false;
        let Some(modal) = self.birthday_modal.take() else {
            return;
        };
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal_view(modal, cx));
    }

    fn leave(&mut self, cx: &mut Context<Self>) {
        let clan_id = self.clan_id;
        self.dismiss_birthday_prompt(cx);
        navigate(cx, Route::ClanMembers { clan_id });
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let channel_id = self.channel_id;
        self.settings.update(cx, |settings, cx| {
            if settings.age_restricted_confirmed.contains(&channel_id) {
                return;
            }
            settings.age_restricted_confirmed.push(channel_id);
            cx.notify();
        });
        schedule_settings_save(&self.settings, cx);
        self.dismiss_birthday_prompt(cx);
    }
}

impl Render for AgeRestrictedGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let text_color = cx.theme().tokens.text_theme_primary;
        let active_color = cx.theme().tokens.text_secondary;
        let border_color = cx.theme().tokens.border_primary;

        div()
            .size_full()
            .flex()
            .justify_center()
            .items_center()
            .text_color(text_color)
            .child(
                v_flex()
                    .max_w_full()
                    .items_center()
                    .child(
                        img(AGE_RESTRICTED_WARNING)
                            .w(px(WARNING_SIZE))
                            .h(px(WARNING_SIZE))
                            .flex_none(),
                    )
                    .child(
                        v_flex()
                            .max_w_full()
                            .mt(px(16.))
                            .items_center()
                            .text_center()
                            .child(
                                div()
                                    .max_w_full()
                                    .text_size(px(30.))
                                    .line_height(px(36.))
                                    .font_weight(FontWeight::BOLD)
                                    .mb(px(8.))
                                    .text_color(active_color)
                                    .child(mezon_i18n::t(&locale, "ageRestricted.title")),
                            )
                            .child(
                                div()
                                    .max_w_full()
                                    .mb(px(16.))
                                    .child(mezon_i18n::t(&locale, "ageRestricted.description")),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(16.))
                            .child(
                                div()
                                    .id("age-restricted-nope")
                                    .px(px(24.))
                                    .py(px(8.))
                                    .rounded(px(8.))
                                    .border_2()
                                    .border_color(border_color)
                                    .text_color(active_color)
                                    .cursor_pointer()
                                    .child(mezon_i18n::t(&locale, "ageRestricted.nope"))
                                    .on_click(cx.listener(|this, _, _window, cx| this.leave(cx))),
                            )
                            .when(viewer_needs_birthday(cx), |row| {
                                row.child(
                                    div()
                                        .id("age-restricted-enter-birthday")
                                        .px(px(24.))
                                        .py(px(8.))
                                        .rounded(px(8.))
                                        .border_2()
                                        .border_color(border_color)
                                        .text_color(active_color)
                                        .cursor_pointer()
                                        .child(mezon_i18n::t(
                                            &locale,
                                            "ageRestricted.enterBirthday",
                                        ))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.reopen_birthday_prompt(window, cx)
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .id("age-restricted-continue")
                                    .px(px(24.))
                                    .py(px(8.))
                                    .rounded(px(8.))
                                    .border_2()
                                    .border_color(rgb(COLOR_DANGER))
                                    .bg(rgb(COLOR_DANGER))
                                    .text_color(rgb(FIELD_TEXT_ACTIVE))
                                    .cursor_pointer()
                                    .child(mezon_i18n::t(&locale, "ageRestricted.continue"))
                                    .on_click(cx.listener(|this, _, _window, cx| this.confirm(cx))),
                            ),
                    ),
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BirthdayField {
    Day,
    Month,
    Year,
}

impl BirthdayField {
    fn id(self) -> &'static str {
        match self {
            Self::Day => "birthday-day",
            Self::Month => "birthday-month",
            Self::Year => "birthday-year",
        }
    }

    fn list_id(self) -> &'static str {
        match self {
            Self::Day => "birthday-day-items",
            Self::Month => "birthday-month-items",
            Self::Year => "birthday-year-items",
        }
    }

    fn placeholder_key(self) -> &'static str {
        match self {
            Self::Day => "ageRestricted.selectDay",
            Self::Month => "ageRestricted.selectMonth",
            Self::Year => "ageRestricted.selectYear",
        }
    }
}

pub struct ConfirmBirthdayModal {
    focus_handle: FocusHandle,
    settings: Entity<Settings>,
    days: Rc<Vec<SharedString>>,
    months: Rc<Vec<SharedString>>,
    years: Rc<Vec<SharedString>>,
    dob_label: SharedString,
    first_year: i32,
    selected_day: Option<usize>,
    selected_month: Option<usize>,
    selected_year: Option<usize>,
    open_field: Option<BirthdayField>,
    submitting: bool,
    _account_sub: Option<Subscription>,
}

impl ConfirmBirthdayModal {
    fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let locale = settings.read(cx).language.clone();
        let days = (1..=31).map(|day| day.to_string().into()).collect();
        let months = MONTH_KEYS
            .iter()
            .map(|key| mezon_i18n::t(&locale, key).into())
            .collect();
        let newest_year = Utc::now().year() - 1;
        let oldest_year = (newest_year - BIRTH_YEAR_SPAN).max(EARLIEST_BIRTH_YEAR);
        let years = (oldest_year..=newest_year)
            .rev()
            .map(|year| year.to_string().into())
            .collect();

        let account_sub = AccountStore::try_global(cx).map(|store| {
            cx.subscribe(
                &store,
                |this: &mut Self, _, event: &AccountEvent, cx| match event {
                    AccountEvent::DateOfBirthSaved => {
                        this.submitting = false;
                        let modal = cx.entity_id();
                        Shell::global(cx).update(cx, |shell, cx| shell.close_modal_view(modal, cx));
                    }
                    AccountEvent::DateOfBirthSaveFailed(_) => {
                        this.submitting = false;
                        let message = mezon_i18n::t(
                            &this.settings.read(cx).language,
                            "common.somethingWentWrong",
                        );
                        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                        cx.notify();
                    }
                    _ => {}
                },
            )
        });

        Self {
            focus_handle: cx.focus_handle(),
            settings,
            days: Rc::new(days),
            months: Rc::new(months),
            years: Rc::new(years),
            dob_label: mezon_i18n::t(&locale, "ageRestricted.dateOfBirth")
                .to_uppercase()
                .into(),
            first_year: newest_year,
            selected_day: None,
            selected_month: None,
            selected_year: None,
            open_field: None,
            submitting: false,
            _account_sub: account_sub,
        }
    }

    fn items(&self, field: BirthdayField) -> Rc<Vec<SharedString>> {
        match field {
            BirthdayField::Day => self.days.clone(),
            BirthdayField::Month => self.months.clone(),
            BirthdayField::Year => self.years.clone(),
        }
    }

    fn selected_index(&self, field: BirthdayField) -> Option<usize> {
        match field {
            BirthdayField::Day => self.selected_day,
            BirthdayField::Month => self.selected_month,
            BirthdayField::Year => self.selected_year,
        }
    }

    fn selected_date(&self) -> Option<NaiveDate> {
        birthday_from_parts(
            self.first_year,
            self.selected_day,
            self.selected_month,
            self.selected_year,
        )
    }

    fn toggle_field(&mut self, field: BirthdayField, cx: &mut Context<Self>) {
        self.open_field = if self.open_field == Some(field) {
            None
        } else {
            Some(field)
        };
        cx.notify();
    }

    fn select(&mut self, field: BirthdayField, index: usize, cx: &mut Context<Self>) {
        match field {
            BirthdayField::Day => self.selected_day = Some(index),
            BirthdayField::Month => self.selected_month = Some(index),
            BirthdayField::Year => self.selected_year = Some(index),
        }
        self.open_field = None;
        cx.notify();
    }

    fn close_fields(&mut self, cx: &mut Context<Self>) {
        if self.open_field.take().is_some() {
            cx.notify();
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        let Some(date) = self.selected_date() else {
            return;
        };
        let Some(seconds) = date
            .and_hms_opt(0, 0, 0)
            .map(|midnight| midnight.and_utc().timestamp())
        else {
            return;
        };
        let Ok(dob_seconds) = u32::try_from(seconds) else {
            return;
        };
        let Some(accounts) = AccountStore::try_global(cx) else {
            return;
        };
        accounts.update(cx, |store, cx| {
            store.save_date_of_birth(dob_seconds, cx);
        });
        self.submitting = true;
        cx.notify();
    }

    fn render_field(
        &self,
        field: BirthdayField,
        locale: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = self.items(field);
        let selected = self.selected_index(field);
        let open = self.open_field == Some(field);
        let label = selected
            .and_then(|index| items.get(index).cloned())
            .unwrap_or_else(|| mezon_i18n::t(locale, field.placeholder_key()).into());
        let label_color = if selected.is_some() {
            rgb(FIELD_TEXT_ACTIVE)
        } else {
            rgb(FIELD_TEXT)
        };
        let list_height = (items.len() as f32 * FIELD_ITEM_HEIGHT).min(FIELD_LIST_MAX_HEIGHT);
        let this = cx.entity().downgrade();

        div()
            .relative()
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .id(field.id())
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(12.))
                    .rounded(px(6.))
                    .bg(rgb(FIELD_BG))
                    .border_1()
                    .border_color(rgb(FIELD_BORDER))
                    .text_color(label_color)
                    .cursor_pointer()
                    .child(div().min_w_0().truncate().child(label))
                    .child(
                        Icon::new(IconName::ChevronDownThin)
                            .w(px(12.))
                            .h(px(8.))
                            .flex_none()
                            .text_color(rgb(FIELD_TEXT)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_field(field, cx);
                        }),
                    ),
            )
            .when(open, |el| {
                el.child(deferred(
                    div()
                        .absolute()
                        .top_full()
                        .mt(px(4.))
                        .left_0()
                        .right_0()
                        .p(px(8.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(rgb(FIELD_BORDER))
                        .bg(rgb(FIELD_BG))
                        .text_color(rgb(FIELD_TEXT))
                        .shadow_lg()
                        .occlude()
                        .child(
                            uniform_list(
                                field.list_id(),
                                items.len(),
                                move |range, _window, _cx| {
                                    range
                                        .map(|index| {
                                            let this = this.clone();
                                            let label = items[index].clone();
                                            div()
                                                .id(("birthday-option", index))
                                                .w_full()
                                                .h(px(FIELD_ITEM_HEIGHT))
                                                .flex()
                                                .items_center()
                                                .px(px(8.))
                                                .rounded(px(8.))
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(FIELD_ITEM_HOVER)))
                                                .child(label)
                                                .on_click(move |_, _window, cx| {
                                                    this.update(cx, |this, cx| {
                                                        this.select(field, index, cx);
                                                    })
                                                    .ok();
                                                })
                                        })
                                        .collect()
                                },
                            )
                            .w_full()
                            .h(px(list_height)),
                        ),
                ))
            })
            .into_any_element()
    }
}

impl Render for ConfirmBirthdayModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let card_bg = cx.theme().tokens.theme_setting_primary;
        let text_color = cx.theme().tokens.text_theme_primary;
        let active_color = cx.theme().tokens.text_secondary;
        let can_submit = self.selected_date().is_some() && !self.submitting;

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .w(px(CARD_WIDTH))
            .items_center()
            .pt(px(16.))
            .rounded(px(4.))
            .bg(card_bg)
            .text_color(text_color)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close_fields(cx)),
            )
            .child(
                img(AGE_RESTRICTED_WARNING)
                    .w(px(WARNING_SIZE))
                    .h(px(WARNING_SIZE))
                    .flex_none(),
            )
            .child(
                v_flex()
                    .mx(px(24.))
                    .items_center()
                    .text_center()
                    .child(
                        div()
                            .text_size(px(24.))
                            .line_height(px(32.))
                            .font_weight(FontWeight::BOLD)
                            .mb(px(16.))
                            .text_color(active_color)
                            .child(mezon_i18n::t(&locale, "ageRestricted.confirmBirthdayTitle")),
                    )
                    .child(div().child(mezon_i18n::t(
                        &locale,
                        "ageRestricted.confirmBirthdayMessage",
                    ))),
            )
            .child(
                v_flex()
                    .w(relative(0.9))
                    .gap(px(8.))
                    .mt(px(20.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(active_color)
                            .child(self.dob_label.clone()),
                    )
                    .child(
                        h_flex()
                            .gap(px(12.))
                            .pb(px(16.))
                            .child(self.render_field(BirthdayField::Day, &locale, cx))
                            .child(self.render_field(BirthdayField::Month, &locale, cx))
                            .child(self.render_field(BirthdayField::Year, &locale, cx)),
                    ),
            )
            .child(
                h_flex().w(relative(0.9)).mb(px(16.)).child(
                    div()
                        .id("birthday-submit")
                        .w_full()
                        .px(px(24.))
                        .py(px(8.))
                        .rounded(px(8.))
                        .border_2()
                        .text_center()
                        .when(can_submit, |el| {
                            el.border_color(rgb(SUBMIT_BG))
                                .bg(rgb(SUBMIT_BG))
                                .text_color(rgb(FIELD_TEXT_ACTIVE))
                                .cursor_pointer()
                        })
                        .when(!can_submit, |el| {
                            el.border_color(rgb(SUBMIT_DISABLED_BG))
                                .bg(rgb(SUBMIT_DISABLED_BG))
                                .text_color(rgb(SUBMIT_DISABLED_TEXT))
                        })
                        .child(mezon_i18n::t(&locale, "ageRestricted.submit"))
                        .on_click(cx.listener(|this, _, _window, cx| this.submit(cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgeRestrictedGate, BirthdayField, ConfirmBirthdayModal, birthday_from_parts,
        gated_channel_type,
    };
    use chrono::{Datelike, NaiveDate, Utc};
    use gpui::{IntoElement, prelude::*};
    use gpui::{TestAppContext, point, px, size};
    use mezon_store::{ChannelId, ChannelType, ClanId, Settings};

    #[test]
    fn only_flagged_channels_are_gated() {
        assert!(gated_channel_type(1, ChannelType::Text));
        assert!(!gated_channel_type(0, ChannelType::Text));
    }

    #[test]
    fn voice_and_stream_channels_are_never_gated() {
        assert!(!gated_channel_type(1, ChannelType::Voice));
        assert!(!gated_channel_type(1, ChannelType::Stream));
        assert!(gated_channel_type(1, ChannelType::App));
        assert!(gated_channel_type(1, ChannelType::Thread));
    }

    #[test]
    fn birthday_needs_all_three_parts() {
        assert_eq!(birthday_from_parts(2025, None, Some(0), Some(0)), None);
        assert_eq!(birthday_from_parts(2025, Some(0), None, Some(0)), None);
        assert_eq!(birthday_from_parts(2025, Some(0), Some(0), None), None);
    }

    #[test]
    fn birthday_maps_indexes_to_a_date() {
        assert_eq!(
            birthday_from_parts(2025, Some(14), Some(6), Some(25)),
            NaiveDate::from_ymd_opt(2000, 7, 15)
        );
        assert_eq!(
            birthday_from_parts(2025, Some(0), Some(0), Some(0)),
            NaiveDate::from_ymd_opt(2025, 1, 1)
        );
    }

    #[test]
    fn birthday_rejects_impossible_days() {
        assert_eq!(birthday_from_parts(2025, Some(30), Some(1), Some(0)), None);
        assert_eq!(birthday_from_parts(2025, Some(28), Some(1), Some(0)), None);
        assert_eq!(
            birthday_from_parts(2025, Some(28), Some(1), Some(1)),
            NaiveDate::from_ymd_opt(2024, 2, 29)
        );
    }

    #[gpui::test]
    fn gate_draws(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| {
            crate::theme::set_theme(crate::theme::resolve_theme("dark"), cx);
        });
        let settings = cx.update(|_, cx| cx.new(|_| Settings::default()));
        cx.draw(point(px(0.), px(0.)), size(px(1200.), px(800.)), |_, cx| {
            cx.new(|_| AgeRestrictedGate::new(ClanId(1), ChannelId(2), settings.clone()))
                .into_any_element()
        });
    }

    #[gpui::test]
    fn birthday_form_draws(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| {
            crate::theme::set_theme(crate::theme::resolve_theme("dark"), cx);
        });
        let settings = cx.update(|_, cx| cx.new(|_| Settings::default()));
        let modal = cx.update(|_, cx| cx.new(|cx| ConfirmBirthdayModal::new(settings, cx)));
        modal.update(cx, |modal, cx| {
            modal.select(BirthdayField::Day, 14, cx);
            modal.select(BirthdayField::Month, 6, cx);
            modal.select(BirthdayField::Year, 0, cx);
        });
        let expected = NaiveDate::from_ymd_opt(Utc::now().year() - 1, 7, 15);
        assert_eq!(
            modal.read_with(cx, |modal, _| modal.selected_date()),
            expected
        );
        cx.draw(point(px(0.), px(0.)), size(px(1200.), px(800.)), |_, _| {
            modal.clone().into_any_element()
        });
    }

    #[gpui::test]
    fn birthday_form_opens_one_field_at_a_time(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let settings = cx.update(|_, cx| cx.new(|_| Settings::default()));
        let modal = cx.update(|_, cx| cx.new(|cx| ConfirmBirthdayModal::new(settings, cx)));
        modal.update(cx, |modal, cx| {
            modal.toggle_field(BirthdayField::Day, cx);
            assert_eq!(modal.open_field, Some(BirthdayField::Day));
            modal.toggle_field(BirthdayField::Year, cx);
            assert_eq!(modal.open_field, Some(BirthdayField::Year));
            modal.toggle_field(BirthdayField::Year, cx);
            assert_eq!(modal.open_field, None);
            modal.toggle_field(BirthdayField::Month, cx);
            modal.select(BirthdayField::Month, 3, cx);
            assert_eq!(modal.open_field, None);
            assert_eq!(modal.selected_month, Some(3));
        });
    }

    #[test]
    fn age_gate_strings_are_translated_in_every_locale() {
        const LOCALES: [&str; 16] = [
            "en", "vi", "ru", "ukr", "es", "tt", "de", "it", "pt", "jpn", "pl", "kr", "swe", "blr",
            "fr", "nl",
        ];
        const KEYS: [&str; 8] = [
            "ageRestricted.title",
            "ageRestricted.description",
            "ageRestricted.nope",
            "ageRestricted.continue",
            "ageRestricted.enterBirthday",
            "ageRestricted.confirmBirthdayTitle",
            "ageRestricted.dateOfBirth",
            "ageRestricted.submit",
        ];
        for locale in LOCALES {
            for key in KEYS {
                let label = mezon_i18n::t(locale, key);
                assert_ne!(label, key, "{locale} is missing {key}");
                assert!(!label.is_empty(), "{locale} has an empty {key}");
            }
        }
    }
}
