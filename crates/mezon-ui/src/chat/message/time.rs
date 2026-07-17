use chrono::{DateTime, Datelike, Duration, Local, NaiveDate};
use gpui::SharedString;
use mezon_store::message_time::local_datetime;
use ui::utils::{DateTimeType, format_distance};

const MONTH_KEYS: [&str; 12] = [
    "common.timeFormat.months.jan",
    "common.timeFormat.months.feb",
    "common.timeFormat.months.mar",
    "common.timeFormat.months.apr",
    "common.timeFormat.months.may",
    "common.timeFormat.months.jun",
    "common.timeFormat.months.jul",
    "common.timeFormat.months.aug",
    "common.timeFormat.months.sep",
    "common.timeFormat.months.oct",
    "common.timeFormat.months.nov",
    "common.timeFormat.months.dec",
];

const WEEKDAY_KEYS: [&str; 7] = [
    "common.timeFormat.daysOfWeek.sun",
    "common.timeFormat.daysOfWeek.mon",
    "common.timeFormat.daysOfWeek.tue",
    "common.timeFormat.daysOfWeek.wed",
    "common.timeFormat.daysOfWeek.thu",
    "common.timeFormat.daysOfWeek.fri",
    "common.timeFormat.daysOfWeek.sat",
];

pub fn format_message_time(
    time_hhmm: &SharedString,
    local_date: Option<NaiveDate>,
    locale: &str,
    now: DateTime<Local>,
) -> SharedString {
    let Some(msg_date) = local_date else {
        return time_hhmm.clone();
    };

    let today = now.date_naive();
    let yesterday = today - Duration::days(1);

    if msg_date == today {
        time_hhmm.clone()
    } else if msg_date == yesterday {
        format!(
            "{} {}",
            mezon_i18n::t(locale, "common.yesterdayAt"),
            time_hhmm
        )
        .into()
    } else {
        format!(
            "{:02}/{:02}/{}, {}",
            msg_date.day(),
            msg_date.month(),
            msg_date.year(),
            time_hhmm
        )
        .into()
    }
}

pub fn format_relative_time_from_seconds(
    timestamp_sec: i64,
    locale: &str,
    now: DateTime<Local>,
) -> String {
    let timestamp_sec = mezon_store::message_time::normalize_unix_seconds(timestamp_sec);
    if timestamp_sec <= 0 {
        return String::new();
    }
    let Some(target) = local_datetime(timestamp_sec) else {
        return String::new();
    };
    let diff = now.timestamp().saturating_sub(timestamp_sec);
    if diff <= 1 {
        return mezon_i18n::t(locale, "common.justNow").to_string();
    }
    format_distance(
        DateTimeType::Naive(target.naive_local()),
        now.naive_local(),
        false,
        true,
        false,
    )
}

pub fn format_date_divider(ts: i64, locale: &str, now: DateTime<Local>) -> String {
    let Some(dt) = local_datetime(ts) else {
        return String::new();
    };

    let month = mezon_i18n::t(locale, MONTH_KEYS[dt.month0() as usize]);
    let formatted = format!("{:02} {} {}", dt.day(), month, dt.year());

    let today = now.date_naive();
    if dt.date_naive() == today {
        format!("{}, {}", mezon_i18n::t(locale, "common.today"), formatted)
    } else {
        let weekday = mezon_i18n::t(
            locale,
            WEEKDAY_KEYS[dt.weekday().num_days_from_sunday() as usize],
        );
        format!("{weekday}, {formatted}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn message_time_today_is_hhmm_only() {
        let now = Local::now();
        let hhmm: SharedString = format!("{:02}:{:02}", now.hour(), now.minute()).into();
        let label = format_message_time(&hhmm, Some(now.date_naive()), "en", now);
        assert_eq!(label, hhmm);
    }

    #[test]
    fn date_divider_today_prefixes_common_today() {
        let now = Local::now();
        let label = format_date_divider(now.timestamp(), "en", now);
        assert!(label.starts_with("Today,"));
    }
}
