use chrono::{Datelike, Local, NaiveDate, Weekday};
use gpui::{
    App, Context, Entity, EventEmitter, FontWeight, Hsla, MouseButton, MouseDownEvent, Render,
    SharedString, Window, deferred, div, prelude::*, px, svg,
};

use super::icon::{Icon, IconName};
use crate::theme::{ActiveTheme, Theme};

const FIELD_HEIGHT: f32 = 32.0;
const CALENDAR_WIDTH: f32 = 280.0;
const MIN_GALLERY_DATE: (i32, u32, u32) = (2020, 1, 1);

fn surface_bg(theme: &Theme) -> gpui::Rgba {
    theme.tokens.bg_surface
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePickerEvent {
    Change(Option<NaiveDate>),
    Opened,
}

pub struct DatePicker {
    selected: Option<NaiveDate>,
    open: bool,
    view_year: i32,
    view_month: u32,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
    locale: SharedString,
}

impl EventEmitter<DatePickerEvent> for DatePicker {}

impl DatePicker {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let today = Local::now().date_naive();
        Self {
            selected: None,
            open: false,
            view_year: today.year(),
            view_month: today.month(),
            min_date: Some(
                NaiveDate::from_ymd_opt(MIN_GALLERY_DATE.0, MIN_GALLERY_DATE.1, MIN_GALLERY_DATE.2)
                    .unwrap_or(today),
            ),
            max_date: None,
            locale: SharedString::default(),
        }
    }

    pub fn selected(&self) -> Option<NaiveDate> {
        self.selected
    }

    pub fn set_locale(&mut self, locale: impl Into<SharedString>) {
        self.locale = locale.into();
    }

    pub fn set_selected_silent(&mut self, date: Option<NaiveDate>, cx: &mut Context<Self>) {
        self.selected = date;
        if let Some(date) = date {
            self.view_year = date.year();
            self.view_month = date.month();
        }
        cx.notify();
    }

    pub fn set_selected(&mut self, date: Option<NaiveDate>, cx: &mut Context<Self>) {
        self.selected = date;
        if let Some(date) = date {
            self.view_year = date.year();
            self.view_month = date.month();
        }
        cx.emit(DatePickerEvent::Change(date));
        cx.notify();
    }

    pub fn set_min(&mut self, min: Option<NaiveDate>, cx: &mut Context<Self>) {
        self.min_date = min;
        cx.notify();
    }

    pub fn set_max(&mut self, max: Option<NaiveDate>, cx: &mut Context<Self>) {
        self.max_date = max;
        cx.notify();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    fn display_date(&self) -> NaiveDate {
        self.selected.unwrap_or_else(|| Local::now().date_naive())
    }

    fn toggle_open(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        if self.open {
            let display = self.display_date();
            self.view_year = display.year();
            self.view_month = display.month();
            cx.emit(DatePickerEvent::Opened);
        }
        cx.notify();
    }

    fn prev_month(&mut self, cx: &mut Context<Self>) {
        if self.view_month == 1 {
            self.view_month = 12;
            self.view_year -= 1;
        } else {
            self.view_month -= 1;
        }
        cx.notify();
    }

    fn next_month(&mut self, cx: &mut Context<Self>) {
        if self.view_month == 12 {
            self.view_month = 1;
            self.view_year += 1;
        } else {
            self.view_month += 1;
        }
        cx.notify();
    }

    fn pick_day(&mut self, day: u32, cx: &mut Context<Self>) {
        let Some(date) = NaiveDate::from_ymd_opt(self.view_year, self.view_month, day) else {
            return;
        };
        if !self.is_day_enabled(date) {
            return;
        }
        self.selected = Some(date);
        self.open = false;
        cx.emit(DatePickerEvent::Change(Some(date)));
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.open = false;
        cx.emit(DatePickerEvent::Change(None));
        cx.notify();
    }

    fn pick_today(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        if self.is_day_enabled(today) {
            self.set_selected(Some(today), cx);
            self.open = false;
        }
    }

    fn is_day_enabled(&self, date: NaiveDate) -> bool {
        if let Some(min) = self.min_date
            && date < min
        {
            return false;
        }
        if let Some(max) = self.max_date
            && date > max
        {
            return false;
        }
        true
    }
}

impl Render for DatePicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.locale.clone();
        let display = format_date(self.display_date());
        let entity = cx.entity();

        div()
            .relative()
            .w_full()
            .child(
                div()
                    .id(("date-picker-field", entity.entity_id()))
                    .w_full()
                    .h(px(FIELD_HEIGHT))
                    .px_3()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .rounded(px(6.))
                    .bg(surface_bg(&theme))
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.bg_hover))
                    .on_mouse_down(MouseButton::Left, {
                        let entity = entity.clone();
                        move |_: &MouseDownEvent, _window, cx: &mut App| {
                            cx.stop_propagation();
                            entity.update(cx, |this, cx| this.toggle_open(cx));
                        }
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(14.))
                            .text_color(theme.text_primary)
                            .child(display),
                    )
                    .child(
                        svg()
                            .path("icons/date-picker-indicator.svg")
                            .size(px(16.))
                            .flex_none()
                            .text_color(theme.text_secondary),
                    ),
            )
            .when(self.open, |el| {
                el.child(
                    deferred(render_calendar(
                        &theme,
                        &locale,
                        self.view_year,
                        self.view_month,
                        self.selected,
                        self.min_date,
                        self.max_date,
                        entity,
                    ))
                    .with_priority(1),
                )
            })
    }
}

fn render_calendar(
    theme: &Theme,
    locale: &str,
    year: i32,
    month: u32,
    selected: Option<NaiveDate>,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
    entity: Entity<DatePicker>,
) -> impl IntoElement {
    let month_label = month_title(year, month);
    let today = Local::now().date_naive();
    let clear_label = mezon_i18n::t(locale, "channelTopbar.gallery.buttons.clearAll");
    let today_label = mezon_i18n::t(locale, "common.today");

    div()
        .occlude()
        .absolute()
        .top(px(FIELD_HEIGHT + 4.))
        .left_0()
        .w(px(CALENDAR_WIDTH))
        .p_3()
        .flex()
        .flex_col()
        .gap_2()
        .rounded(px(8.))
        .bg(surface_bg(theme))
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down_out({
            let entity = entity.clone();
            move |_: &MouseDownEvent, _window, cx: &mut App| {
                entity.update(cx, |this, cx| this.close(cx));
            }
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(month_label),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(calendar_nav_button(IconName::ArrowDown, theme, true, {
                            let entity = entity.clone();
                            move |_: &MouseDownEvent, _window, cx: &mut App| {
                                cx.stop_propagation();
                                entity.update(cx, |this, cx| this.prev_month(cx));
                            }
                        }))
                        .child(calendar_nav_button(IconName::ArrowDown, theme, false, {
                            let entity = entity.clone();
                            move |_: &MouseDownEvent, _window, cx: &mut App| {
                                cx.stop_propagation();
                                entity.update(cx, |this, cx| this.next_month(cx));
                            }
                        })),
                ),
        )
        .child(render_weekday_header(theme))
        .children(render_day_rows(
            theme,
            year,
            month,
            selected,
            today,
            min_date,
            max_date,
            entity.clone(),
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .pt_1()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.text_secondary)
                        .cursor_pointer()
                        .hover(|el| el.text_color(theme.interactive_hover))
                        .child(clear_label)
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_: &MouseDownEvent, _window, cx: &mut App| {
                                cx.stop_propagation();
                                entity.update(cx, |this, cx| this.clear(cx));
                            }
                        }),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.brand)
                        .cursor_pointer()
                        .hover(|el| el.text_color(theme.brand_hover))
                        .child(today_label)
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_: &MouseDownEvent, _window, cx: &mut App| {
                                cx.stop_propagation();
                                entity.update(cx, |this, cx| this.pick_today(cx));
                            }
                        }),
                ),
        )
}

fn calendar_nav_button(
    icon: IconName,
    theme: &Theme,
    rotate_up: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut icon_el = Icon::new(icon)
        .size(px(12.))
        .text_color(theme.text_secondary);
    if rotate_up {
        icon_el = icon_el.with_transformation(gpui::Transformation::rotate(gpui::radians(
            std::f32::consts::PI,
        )));
    }
    div()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|el| el.bg(theme.bg_hover))
        .child(icon_el)
        .on_mouse_down(MouseButton::Left, on_click)
}

fn render_weekday_header(theme: &Theme) -> impl IntoElement {
    let labels = ["S", "M", "T", "W", "T", "F", "S"];
    div()
        .grid()
        .grid_cols(7)
        .gap_0()
        .children(labels.into_iter().map(|label| {
            div()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .text_color(theme.text_secondary)
                .child(label)
        }))
}

fn render_day_rows(
    theme: &Theme,
    year: i32,
    month: u32,
    selected: Option<NaiveDate>,
    today: NaiveDate,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
    entity: Entity<DatePicker>,
) -> Vec<gpui::AnyElement> {
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let days_in_month = days_in_month(year, month);
    let leading = weekday_offset(first.weekday());
    let mut day = 1u32;
    let mut rows = Vec::new();

    for _ in 0..6 {
        let mut cells = Vec::new();
        for col in 0..7 {
            let cell_index = rows.len() * 7 + col;
            if cell_index < leading || day > days_in_month {
                cells.push(empty_day_cell());
            } else {
                let current_day = day;
                let date = NaiveDate::from_ymd_opt(year, month, current_day).unwrap();
                let enabled = is_day_in_range(date, min_date, max_date);
                let is_selected = selected == Some(date);
                let is_today = date == today;
                cells.push(day_cell(
                    theme,
                    current_day,
                    enabled,
                    is_selected,
                    is_today,
                    {
                        let entity = entity.clone();
                        move |_: &MouseDownEvent, _window, cx: &mut App| {
                            cx.stop_propagation();
                            entity.update(cx, |this, cx| this.pick_day(current_day, cx));
                        }
                    },
                ));
                day += 1;
            }
        }
        rows.push(
            div()
                .grid()
                .grid_cols(7)
                .gap_0()
                .children(cells)
                .into_any_element(),
        );
        if day > days_in_month {
            break;
        }
    }
    rows
}

fn is_day_in_range(
    date: NaiveDate,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
) -> bool {
    if let Some(min) = min_date
        && date < min
    {
        return false;
    }
    if let Some(max) = max_date
        && date > max
    {
        return false;
    }
    true
}

fn empty_day_cell() -> gpui::AnyElement {
    div().h(px(32.)).into_any_element()
}

fn day_cell(
    theme: &Theme,
    day: u32,
    enabled: bool,
    selected: bool,
    today: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    let text_color = if !enabled {
        Hsla::from(theme.text_muted)
    } else if selected {
        gpui::white()
    } else if today {
        Hsla::from(theme.brand)
    } else {
        Hsla::from(theme.text_primary)
    };

    let mut cell = div()
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.))
        .text_color(text_color)
        .child(day.to_string());

    if selected {
        cell = cell
            .rounded(px(4.))
            .bg(theme.brand)
            .text_color(gpui::white());
    } else if !enabled {
        cell = cell.opacity(0.35);
    } else {
        cell = cell
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|el| el.bg(theme.bg_hover))
            .on_mouse_down(MouseButton::Left, on_click);
    }

    cell.into_any_element()
}

fn weekday_offset(weekday: Weekday) -> usize {
    match weekday {
        Weekday::Sun => 0,
        Weekday::Mon => 1,
        Weekday::Tue => 2,
        Weekday::Wed => 3,
        Weekday::Thu => 4,
        Weekday::Fri => 5,
        Weekday::Sat => 6,
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap();
    first_next.pred_opt().unwrap().day()
}

fn month_title(year: i32, month: u32) -> String {
    NaiveDate::from_ymd_opt(year, month, 1)
        .map(|date| date.format("%B %Y").to_string())
        .unwrap_or_default()
}

fn format_date(date: NaiveDate) -> String {
    date.format("%d/%m/%Y").to_string()
}
