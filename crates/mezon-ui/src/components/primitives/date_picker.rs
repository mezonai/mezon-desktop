use chrono::{Datelike, Local, NaiveDate, Weekday};
use gpui::{
    App, Context, Entity, EventEmitter, FontWeight, Hsla, MouseButton, MouseDownEvent, Render,
    SharedString, Window, anchored, deferred, div, point, prelude::*, px, svg,
};

use super::icon::{Icon, IconName};
use crate::theme::{ActiveTheme, Theme};

const DEFAULT_FIELD_HEIGHT: f32 = 32.0;
const CALENDAR_WIDTH: f32 = 280.0;
const MIN_GALLERY_DATE: (i32, u32, u32) = (2020, 1, 1);
const YEAR_PAGE: i32 = 12;
const WEEKDAY_ROW_HEIGHT: f32 = 28.0;
const DAY_CELL_HEIGHT: f32 = 32.0;
const CALENDAR_BODY_HEIGHT: f32 = WEEKDAY_ROW_HEIGHT + DAY_CELL_HEIGHT * 6.0;
const OPTION_GRID_GAP: f32 = 4.0;
const OPTION_CELL_HEIGHT: f32 = (CALENDAR_BODY_HEIGHT - OPTION_GRID_GAP * 3.0) / 4.0;

fn surface_bg(theme: &Theme) -> gpui::Rgba {
    theme.tokens.bg_surface
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DatePickerPopupMode {
    #[default]
    DeferredOverlay,
    InlineExpand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CalendarView {
    #[default]
    Days,
    Months,
    Years,
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
    view_mode: CalendarView,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
    locale: SharedString,
    empty_label: Option<SharedString>,
    field_height: f32,
    popup_mode: DatePickerPopupMode,
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
            view_mode: CalendarView::Days,
            min_date: Some(
                NaiveDate::from_ymd_opt(MIN_GALLERY_DATE.0, MIN_GALLERY_DATE.1, MIN_GALLERY_DATE.2)
                    .unwrap_or(today),
            ),
            max_date: None,
            locale: SharedString::default(),
            empty_label: None,
            field_height: DEFAULT_FIELD_HEIGHT,
            popup_mode: DatePickerPopupMode::default(),
        }
    }

    pub fn set_popup_mode(&mut self, mode: DatePickerPopupMode, cx: &mut Context<Self>) {
        if self.popup_mode != mode {
            self.popup_mode = mode;
            cx.notify();
        }
    }

    pub fn popup_mode(&self) -> DatePickerPopupMode {
        self.popup_mode
    }

    pub fn set_empty_label(&mut self, label: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.empty_label = Some(label.into());
        cx.notify();
    }

    pub fn set_field_height(&mut self, height: f32, cx: &mut Context<Self>) {
        self.field_height = height.max(1.0);
        cx.notify();
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
            self.view_mode = CalendarView::Days;
            cx.notify();
        }
    }

    fn display_date(&self) -> NaiveDate {
        self.selected.unwrap_or_else(|| Local::now().date_naive())
    }

    fn toggle_open(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        self.view_mode = CalendarView::Days;
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

    fn step_view(&mut self, forward: bool, cx: &mut Context<Self>) {
        match self.view_mode {
            CalendarView::Days => {
                if forward {
                    self.next_month(cx);
                } else {
                    self.prev_month(cx);
                }
            }
            CalendarView::Months => {
                self.view_year += if forward { 1 } else { -1 };
                cx.notify();
            }
            CalendarView::Years => {
                self.view_year += if forward { YEAR_PAGE } else { -YEAR_PAGE };
                cx.notify();
            }
        }
    }

    fn zoom_out_view(&mut self, cx: &mut Context<Self>) {
        self.view_mode = match self.view_mode {
            CalendarView::Days => CalendarView::Months,
            CalendarView::Months | CalendarView::Years => CalendarView::Years,
        };
        cx.notify();
    }

    fn pick_month(&mut self, month: u32, cx: &mut Context<Self>) {
        self.view_month = month;
        self.view_mode = CalendarView::Days;
        cx.notify();
    }

    fn pick_year(&mut self, year: i32, cx: &mut Context<Self>) {
        self.view_year = year;
        self.view_mode = CalendarView::Months;
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
        let display = match (self.selected, self.empty_label.clone()) {
            (None, Some(label)) => label,
            _ => SharedString::from(format_date(self.display_date())),
        };
        let entity = cx.entity();
        let field_height = self.field_height;

        div()
            .relative()
            .w_full()
            .child(
                div()
                    .id(("date-picker-field", entity.entity_id()))
                    .w_full()
                    .h(px(field_height))
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
            .when(self.open, |el| match self.popup_mode {
                DatePickerPopupMode::DeferredOverlay => el.child(deferred(
                    anchored()
                        .snap_to_window_with_margin(px(8.))
                        .offset(point(px(0.), px(field_height + 4.)))
                        .child(render_calendar(
                            &theme,
                            &locale,
                            self.view_year,
                            self.view_month,
                            self.view_mode,
                            self.selected,
                            self.min_date,
                            self.max_date,
                            entity,
                            false,
                        )),
                )),
                DatePickerPopupMode::InlineExpand => el.child(render_calendar(
                    &theme,
                    &locale,
                    self.view_year,
                    self.view_month,
                    self.view_mode,
                    self.selected,
                    self.min_date,
                    self.max_date,
                    entity,
                    true,
                )),
            })
    }
}

#[allow(clippy::too_many_arguments)]
fn render_calendar(
    theme: &Theme,
    locale: &str,
    year: i32,
    month: u32,
    view_mode: CalendarView,
    selected: Option<NaiveDate>,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
    entity: Entity<DatePicker>,
    inline: bool,
) -> impl IntoElement {
    let header_label = match view_mode {
        CalendarView::Days => month_title(year, month),
        CalendarView::Months => year.to_string(),
        CalendarView::Years => {
            let start = year_page_start(year);
            format!("{start} – {}", start + YEAR_PAGE - 1)
        }
    };
    let today = Local::now().date_naive();
    let clear_label = mezon_i18n::t(locale, "channelTopbar.gallery.buttons.clearAll");
    let today_label = mezon_i18n::t(locale, "common.today");

    let mut root = div()
        .occlude()
        .p_3()
        .flex()
        .flex_col()
        .gap_2()
        .rounded(px(8.))
        .bg(surface_bg(theme))
        .border_1()
        .border_color(theme.border)
        .shadow_lg();
    root = if inline {
        root.w_full().mt(px(4.))
    } else {
        root.w(px(CALENDAR_WIDTH))
    };
    root.on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
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
                    .id("date-picker-header")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.bg_hover))
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(header_label)
                    .child(
                        Icon::new(IconName::ArrowDown)
                            .size(px(10.))
                            .text_color(theme.text_secondary),
                    )
                    .on_mouse_down(MouseButton::Left, {
                        let entity = entity.clone();
                        move |_: &MouseDownEvent, _window, cx: &mut App| {
                            cx.stop_propagation();
                            entity.update(cx, |this, cx| this.zoom_out_view(cx));
                        }
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(calendar_nav_button(
                        "date-picker-prev",
                        IconName::ArrowDown,
                        theme,
                        true,
                        {
                            let entity = entity.clone();
                            move |_: &MouseDownEvent, _window, cx: &mut App| {
                                cx.stop_propagation();
                                entity.update(cx, |this, cx| this.step_view(false, cx));
                            }
                        },
                    ))
                    .child(calendar_nav_button(
                        "date-picker-next",
                        IconName::ArrowDown,
                        theme,
                        false,
                        {
                            let entity = entity.clone();
                            move |_: &MouseDownEvent, _window, cx: &mut App| {
                                cx.stop_propagation();
                                entity.update(cx, |this, cx| this.step_view(true, cx));
                            }
                        },
                    )),
            ),
    )
    .child(
        div()
            .h(px(CALENDAR_BODY_HEIGHT))
            .flex()
            .flex_col()
            .children(match view_mode {
                CalendarView::Days => {
                    let mut rows = vec![render_weekday_header(theme).into_any_element()];
                    rows.extend(
                        render_day_rows(
                            theme,
                            year,
                            month,
                            selected,
                            today,
                            min_date,
                            max_date,
                            entity.clone(),
                        )
                        .into_iter()
                        .map(IntoElement::into_any_element),
                    );
                    rows
                }
                CalendarView::Months => vec![
                    render_month_grid(theme, year, selected, min_date, max_date, entity.clone())
                        .into_any_element(),
                ],
                CalendarView::Years => vec![
                    render_year_grid(theme, year, selected, min_date, max_date, entity.clone())
                        .into_any_element(),
                ],
            }),
    )
    .child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pt_1()
            .child(
                div()
                    .id("date-picker-clear")
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
                    .id("date-picker-today")
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
    id: &'static str,
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
        .id(id)
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
    div().h(px(DAY_CELL_HEIGHT)).into_any_element()
}

fn render_month_grid(
    theme: &Theme,
    year: i32,
    selected: Option<NaiveDate>,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
    entity: Entity<DatePicker>,
) -> impl IntoElement {
    let picker = entity.clone();
    div()
        .grid()
        .grid_cols(3)
        .gap(px(OPTION_GRID_GAP))
        .children((1..=12u32).map(move |month| {
            let enabled = month_in_range(year, month, min_date, max_date);
            let is_selected =
                selected.is_some_and(|date| date.year() == year && date.month() == month);
            let entity = picker.clone();
            option_cell(
                ("date-picker-month", month as u64).into(),
                theme,
                short_month_label(month),
                enabled,
                is_selected,
                move |_: &MouseDownEvent, _window, cx: &mut App| {
                    cx.stop_propagation();
                    entity.update(cx, |this, cx| this.pick_month(month, cx));
                },
            )
        }))
}

fn render_year_grid(
    theme: &Theme,
    year: i32,
    selected: Option<NaiveDate>,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
    entity: Entity<DatePicker>,
) -> impl IntoElement {
    let start = year_page_start(year);
    let picker = entity.clone();
    div()
        .grid()
        .grid_cols(3)
        .gap(px(OPTION_GRID_GAP))
        .children((start..start + YEAR_PAGE).map(move |candidate| {
            let enabled = year_in_range(candidate, min_date, max_date);
            let is_selected = selected.is_some_and(|date| date.year() == candidate);
            let entity = picker.clone();
            option_cell(
                ("date-picker-year", i64::from(candidate).unsigned_abs()).into(),
                theme,
                candidate.to_string(),
                enabled,
                is_selected,
                move |_: &MouseDownEvent, _window, cx: &mut App| {
                    cx.stop_propagation();
                    entity.update(cx, |this, cx| this.pick_year(candidate, cx));
                },
            )
        }))
}

fn option_cell(
    id: gpui::ElementId,
    theme: &Theme,
    label: String,
    enabled: bool,
    selected: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    let mut cell = div()
        .id(id)
        .h(px(OPTION_CELL_HEIGHT))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .text_size(px(13.))
        .text_color(Hsla::from(theme.text_primary))
        .child(label);
    if selected {
        cell = cell.bg(theme.brand).text_color(gpui::white());
    } else if !enabled {
        cell = cell.text_color(Hsla::from(theme.text_muted)).opacity(0.35);
    } else {
        cell = cell
            .cursor_pointer()
            .hover(|el| el.bg(theme.bg_hover))
            .on_mouse_down(MouseButton::Left, on_click);
    }
    cell.into_any_element()
}

fn year_page_start(year: i32) -> i32 {
    year - year.rem_euclid(YEAR_PAGE)
}

fn short_month_label(month: u32) -> String {
    NaiveDate::from_ymd_opt(2000, month, 1)
        .map(|date| date.format("%b").to_string())
        .unwrap_or_default()
}

fn month_in_range(
    year: i32,
    month: u32,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
) -> bool {
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return false;
    };
    let last = first + chrono::Duration::days(days_in_month(year, month) as i64 - 1);
    range_overlaps(first, last, min_date, max_date)
}

fn year_in_range(year: i32, min_date: Option<NaiveDate>, max_date: Option<NaiveDate>) -> bool {
    let (Some(first), Some(last)) = (
        NaiveDate::from_ymd_opt(year, 1, 1),
        NaiveDate::from_ymd_opt(year, 12, 31),
    ) else {
        return false;
    };
    range_overlaps(first, last, min_date, max_date)
}

fn range_overlaps(
    first: NaiveDate,
    last: NaiveDate,
    min_date: Option<NaiveDate>,
    max_date: Option<NaiveDate>,
) -> bool {
    if min_date.is_some_and(|min| last < min) {
        return false;
    }
    if max_date.is_some_and(|max| first > max) {
        return false;
    }
    true
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
        .id(("date-picker-day", day as u64))
        .h(px(DAY_CELL_HEIGHT))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn walking_the_three_views_lands_on_the_picked_month_and_year(cx: &mut gpui::TestAppContext) {
        let picker = cx.new(DatePicker::new);
        picker.update(cx, |picker, cx| {
            picker.set_selected_silent(NaiveDate::from_ymd_opt(2026, 8, 24), cx);
            picker.set_min(None, cx);
            picker.toggle_open(cx);

            assert_eq!(picker.view_mode, CalendarView::Days);
            assert_eq!((picker.view_year, picker.view_month), (2026, 8));

            picker.step_view(true, cx);
            assert_eq!((picker.view_year, picker.view_month), (2026, 9));
            picker.step_view(false, cx);
            assert_eq!((picker.view_year, picker.view_month), (2026, 8));

            picker.zoom_out_view(cx);
            assert_eq!(picker.view_mode, CalendarView::Months);
            picker.step_view(false, cx);
            assert_eq!(picker.view_year, 2025);

            picker.zoom_out_view(cx);
            assert_eq!(picker.view_mode, CalendarView::Years);
            picker.step_view(false, cx);
            assert_eq!(year_page_start(picker.view_year), 2004);

            picker.pick_year(2019, cx);
            assert_eq!(picker.view_mode, CalendarView::Months);
            assert_eq!(picker.view_year, 2019);

            picker.pick_month(3, cx);
            assert_eq!(picker.view_mode, CalendarView::Days);
            assert_eq!((picker.view_year, picker.view_month), (2019, 3));

            picker.pick_day(15, cx);
            assert_eq!(
                picker.selected,
                NaiveDate::from_ymd_opt(2019, 3, 15),
                "picking a day commits the month and year walked to"
            );
            assert!(!picker.open, "picking a day closes the popup");
        });
    }

    #[gpui::test]
    fn reopening_resets_to_the_day_view(cx: &mut gpui::TestAppContext) {
        let picker = cx.new(DatePicker::new);
        picker.update(cx, |picker, cx| {
            picker.toggle_open(cx);
            picker.zoom_out_view(cx);
            picker.zoom_out_view(cx);
            assert_eq!(picker.view_mode, CalendarView::Years);
            picker.close(cx);
            picker.toggle_open(cx);
            assert_eq!(picker.view_mode, CalendarView::Days);
        });
    }

    #[gpui::test]
    fn a_day_outside_the_allowed_range_is_not_picked(cx: &mut gpui::TestAppContext) {
        let picker = cx.new(DatePicker::new);
        picker.update(cx, |picker, cx| {
            picker.set_min(NaiveDate::from_ymd_opt(2026, 8, 10), cx);
            picker.set_max(NaiveDate::from_ymd_opt(2026, 8, 20), cx);
            picker.toggle_open(cx);
            picker.pick_month(8, cx);
            picker.view_year = 2026;
            picker.pick_day(5, cx);
            assert_eq!(picker.selected, None);
            picker.pick_day(15, cx);
            assert_eq!(picker.selected, NaiveDate::from_ymd_opt(2026, 8, 15));
        });
    }

    #[test]
    fn year_pages_are_aligned_blocks_of_twelve() {
        assert_eq!(year_page_start(2026), 2016);
        assert_eq!(year_page_start(2016), 2016);
        assert_eq!(year_page_start(2015), 2004);
    }

    #[test]
    fn month_and_year_cells_respect_the_allowed_range() {
        let min = NaiveDate::from_ymd_opt(2026, 3, 10);
        let max = NaiveDate::from_ymd_opt(2026, 5, 20);

        assert!(!month_in_range(2026, 2, min, max));
        assert!(month_in_range(2026, 3, min, max));
        assert!(month_in_range(2026, 5, min, max));
        assert!(!month_in_range(2026, 6, min, max));

        assert!(!year_in_range(2025, min, max));
        assert!(year_in_range(2026, min, max));
        assert!(!year_in_range(2027, min, max));

        assert!(month_in_range(1970, 1, None, None));
        assert!(year_in_range(3000, None, None));
    }

    #[test]
    fn every_month_renders_six_week_rows() {
        for (year, month) in [(2026, 2), (2026, 8), (2021, 5), (2024, 2)] {
            let first = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
            let cells = weekday_offset(first.weekday()) + days_in_month(year, month) as usize;
            assert!(cells <= 42, "{year}-{month} needs more than six rows");
        }
    }

    #[test]
    fn month_and_day_bodies_are_the_same_height() {
        assert_eq!(
            WEEKDAY_ROW_HEIGHT + DAY_CELL_HEIGHT * 6.0,
            OPTION_CELL_HEIGHT * 4.0 + OPTION_GRID_GAP * 3.0
        );
    }

    #[test]
    fn short_month_labels_cover_every_month() {
        assert_eq!(short_month_label(1), "Jan");
        assert_eq!(short_month_label(12), "Dec");
    }
}
